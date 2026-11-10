import json
import re
import sys
import time
import uuid
from dataclasses import dataclass
from decimal import Decimal

import aiohttp

from coordinator.chain_reader import read_policy, read_venue_address
from coordinator.hub_owner_check import build_owner_cache, hub_owner_refusal
from coordinator.signer_client import ask_signers
from coordinator.venue_digest import action_digest, actions_are_identical, expected_action, expires_after_milliseconds, recover_address
from coordinator.venue_poster import build_signed_body, post_signed_action

MILLISECONDS_PER_SECOND = 1000
HIGHEST_SIGNED_REFUSAL = re.compile(r'nonce_not_above_highest_signed_([0-9]+)')
SUPPORTED_VENUE = 'hyperliquid'
MINIMUM_DEAD_MAN_WINDOW_SECONDS = 30
MAXIMUM_DEAD_MAN_WINDOW_SECONDS = 600
DEFAULT_DEAD_MAN_WINDOW_SECONDS = 300
MINIMUM_REARM_AHEAD_SECONDS = 16
CLOSE_OUT_SECONDS = 600
DEAD_MAN_REQUIRED_ABOVE_USD = Decimal(10000)
VENUE_DEAD_MAN_VOLUME_REFUSAL_PREFIX = 'Cannot set scheduled cancel time until enough volume traded.'


@dataclass(frozen=True)
class CoordinatorConfig:
    listen_host: str
    listen_port: int
    key_ref: str
    signer_urls: tuple[str, ...]
    chain_rest_urls: tuple[str, ...]
    venue_exchange_url: str
    intent_lifetime_seconds: int
    dead_man_window_seconds: int = DEFAULT_DEAD_MAN_WINDOW_SECONDS
    allow_live_hyperliquid_exchange: bool = False
    hub_rest_urls: tuple[str, ...] = ()
    hub_websocket_url: str = ''
    hub_account_id: str = ''


def build_round_state(config: CoordinatorConfig, http_session: aiohttp.ClientSession) -> dict:
    return {
        'config': config,
        'http_session': http_session,
        'venue_address': None,
        'highest_issued_nonce': 0,
        'armed_until_milliseconds_by_vault': {},
        'policy_by_delegate': {},
        'owner_cache': build_owner_cache(),
    }


def now_milliseconds() -> int:
    return int(time.time() * MILLISECONDS_PER_SECOND)


def log_event(event: str, correlation_id: str, **fields) -> None:
    record = {'event_id': uuid.uuid4().hex, 'ts': str(now_milliseconds()), 'source': 'lockbox-coordinator',
              'correlation_id': correlation_id, 'event': event, **{name: str(value) for name, value in fields.items()}}
    print(json.dumps(record), file=sys.stdout, flush=True)


def issue_nonces(state: dict, count: int) -> list[int]:
    first_nonce = max(now_milliseconds(), state['highest_issued_nonce'] + 1)
    state['highest_issued_nonce'] = first_nonce + count - 1
    return [first_nonce + offset for offset in range(count)]


def dead_man_needs_rearming(state: dict, vault_address: str) -> bool:
    armed_until = state['armed_until_milliseconds_by_vault'].get(vault_address, 0)
    half_window = state['config'].dead_man_window_seconds * MILLISECONDS_PER_SECOND // 2
    return armed_until - now_milliseconds() < half_window


def is_new_risk(intent: dict) -> bool:
    return intent.get('kind') == 'order' and intent.get('reduce_only') is not True


def close_out_deadline_milliseconds(policy: dict) -> int:
    ended_at = min(policy['revoked_at'], policy['expires_at']) if policy['revoked'] else policy['expires_at']
    return (ended_at + CLOSE_OUT_SECONDS) * MILLISECONDS_PER_SECOND


def rearm_time_or_none(state: dict, policy: dict) -> int | None:
    latest_wanted = now_milliseconds() + state['config'].dead_man_window_seconds * MILLISECONDS_PER_SECOND
    rearm_time = min(latest_wanted, close_out_deadline_milliseconds(policy))
    earliest_allowed = now_milliseconds() + MINIMUM_REARM_AHEAD_SECONDS * MILLISECONDS_PER_SECOND
    return rearm_time if rearm_time >= earliest_allowed else None


def build_rearm_intent(state: dict, delegate_key: str, vault_address: str, policy_epoch: int, rearm_time: int, venue_nonce: int) -> dict:
    config = state['config']
    return {
        'request_id': uuid.uuid4().hex,
        'key_ref': config.key_ref,
        'delegate_key': delegate_key,
        'policy_epoch': policy_epoch,
        'venue': SUPPORTED_VENUE,
        'vault_address': vault_address,
        'kind': 'schedule_cancel',
        'time_ms': rearm_time,
        'venue_nonce': venue_nonce,
        'expires_at': now_milliseconds() // MILLISECONDS_PER_SECOND + config.intent_lifetime_seconds,
    }


async def prepare_round(state: dict, delegate_key: str, vault_address: str) -> dict:
    config = state['config']
    owner_refusal = hub_owner_refusal(state)
    if owner_refusal is not None:
        log_event('prepare_refused', config.key_ref, reason=owner_refusal, hub_account_id=config.hub_account_id)
        return {'prepared': False, 'reason': owner_refusal}
    policy = await read_policy(state['http_session'], config.chain_rest_urls, config.key_ref, delegate_key)
    if policy is None:
        policy = state['policy_by_delegate'].get(delegate_key)
    if policy is None:
        return {'prepared': False, 'reason': 'policy_unknown'}
    state['policy_by_delegate'][delegate_key] = policy
    rearm_time = rearm_time_or_none(state, policy) if dead_man_needs_rearming(state, vault_address) else None
    nonces = issue_nonces(state, 1 if rearm_time is None else 2)
    rearm_intent = None
    if rearm_time is not None:
        rearm_intent = build_rearm_intent(state, delegate_key, vault_address, policy['epoch'], rearm_time, nonces[0])
    return {
        'prepared': True,
        'key_ref': config.key_ref,
        'policy_epoch': policy['epoch'],
        'venue_nonce': nonces[-1],
        'expires_at': now_milliseconds() // MILLISECONDS_PER_SECOND + config.intent_lifetime_seconds,
        'rearm_intent': rearm_intent,
    }


async def resolve_venue_address(state: dict) -> str | None:
    if state['venue_address'] is None:
        config = state['config']
        state['venue_address'] = await read_venue_address(state['http_session'], config.chain_rest_urls, config.key_ref)
    return state['venue_address']


def raise_nonce_floor_from_refusal(state: dict, reason: str) -> None:
    refusal = HIGHEST_SIGNED_REFUSAL.search(reason)
    if refusal:
        state['highest_issued_nonce'] = max(state['highest_issued_nonce'], int(refusal.group(1)))


def build_expected_or_none(intent: dict) -> dict | None:
    try:
        return expected_action(intent)
    except (KeyError, ValueError, TypeError):
        return None


async def run_signing_round(state: dict, intent: dict) -> dict:
    config = state['config']
    request_id = intent.get('request_id', 'unnamed')
    venue_address = await resolve_venue_address(state)
    if venue_address is None:
        return {'request_id': request_id, 'signed': False, 'reason': 'venue_address_unknown'}
    expected = build_expected_or_none(intent)
    reply = await ask_signers(state['http_session'], config.signer_urls, intent)
    if not reply.get('signed'):
        reason = reply.get('reason', 'no_reason')
        raise_nonce_floor_from_refusal(state, reason)
        log_event('round_refused', request_id, kind=intent.get('kind'), reason=reason)
        return {'request_id': request_id, 'signed': False, 'reason': reason}
    if expected is None or not actions_are_identical(expected, reply.get('action')):
        log_event('round_action_mismatch', request_id)
        return {'request_id': request_id, 'signed': False, 'reason': 'action_is_not_the_expected_one'}
    expires_after = expires_after_milliseconds(intent)
    digest = action_digest(expected, intent['venue_nonce'], intent['vault_address'], expires_after)
    if digest.hex() != reply.get('digest'):
        log_event('round_digest_mismatch', request_id)
        return {'request_id': request_id, 'signed': False, 'reason': 'digest_is_not_the_expected_one'}
    recovered = recover_address(digest, reply['r'], reply['s'], reply['v'])
    if recovered != venue_address:
        log_event('round_wrong_signer', request_id, recovered=recovered)
        return {'request_id': request_id, 'signed': False, 'reason': 'signature_does_not_recover_to_venue_address'}
    body = build_signed_body(expected, intent['venue_nonce'], reply['r'], reply['s'], reply['v'], intent['vault_address'], expires_after)
    venue_status, venue_answer = await post_signed_action(state['http_session'], config.venue_exchange_url, config.allow_live_hyperliquid_exchange, body)
    log_event('round_posted', request_id, kind=intent['kind'], venue_status=venue_status, asked=reply.get('asked'),
              **({'venue_answer': json.dumps(venue_answer)[:400]} if venue_status != 'ok' else {}))
    return {'request_id': request_id, 'signed': True, 'digest': reply['digest'], 'r': reply['r'], 's': reply['s'], 'v': reply['v'],
            'action': expected, 'recovered': recovered, 'venue_status': venue_status, 'venue_answer': venue_answer, 'asked': reply.get('asked')}


def note_dead_man_armed(state: dict, intent: dict, outcome: dict) -> None:
    if intent.get('kind') == 'schedule_cancel' and outcome.get('venue_status') == 'ok':
        state['armed_until_milliseconds_by_vault'][intent['vault_address']] = intent['time_ms']


def venue_refused_dead_man_for_volume(rearm_outcome: dict | None) -> bool:
    answer = (rearm_outcome or {}).get('venue_answer') or {}
    response = answer.get('response') if isinstance(answer, dict) else None
    return (rearm_outcome or {}).get('venue_status') == 'err' and isinstance(response, str) \
        and response.startswith(VENUE_DEAD_MAN_VOLUME_REFUSAL_PREFIX)


def policy_caps_stay_below_dead_man_threshold(state: dict, intent: dict) -> bool:
    policy = state['policy_by_delegate'].get(intent.get('delegate_key'))
    limit_price = intent.get('limit_px')
    if policy is None or not limit_price:
        return False
    caps = policy.get('asset_caps') or []
    asset_name = intent.get('asset')
    matching = [cap for cap in caps if cap.get('asset') == asset_name] if asset_name else caps
    if not matching:
        return False
    largest_position = max(Decimal(cap['max_daily_size']) for cap in matching)
    return largest_position * Decimal(limit_price) < DEAD_MAN_REQUIRED_ABOVE_USD


async def handle_intent(state: dict, intent: dict, rearm_intent: dict | None) -> dict:
    vault_address = intent.get('vault_address', '')
    rearm_outcome = None
    if rearm_intent is not None:
        rearm_outcome = await run_signing_round(state, rearm_intent)
        note_dead_man_armed(state, rearm_intent, rearm_outcome)
    still_needs_rearm = intent.get('kind') != 'schedule_cancel' and dead_man_needs_rearming(state, vault_address)
    if is_new_risk(intent) and still_needs_rearm and state['armed_until_milliseconds_by_vault'].get(vault_address, 0) <= now_milliseconds():
        if venue_refused_dead_man_for_volume(rearm_outcome) and policy_caps_stay_below_dead_man_threshold(state, intent):
            log_event('dead_man_unavailable_volume', intent.get('request_id', 'unnamed'), vault_address=vault_address,
                      threshold_usd=str(DEAD_MAN_REQUIRED_ABOVE_USD))
            outcome = await run_signing_round(state, intent)
            outcome['rearm'] = rearm_outcome
            outcome['dead_man'] = 'unavailable_volume'
            return outcome
        reason = rearm_outcome.get('reason', 'rearm_not_posted') if rearm_outcome else 'rearm_required'
        log_event('new_risk_stopped_no_dead_man', intent.get('request_id', 'unnamed'), reason=reason)
        return {'request_id': intent.get('request_id'), 'signed': False, 'reason': f'dead_man_not_armed: {reason}', 'rearm': rearm_outcome}
    outcome = await run_signing_round(state, intent)
    note_dead_man_armed(state, intent, outcome)
    outcome['rearm'] = rearm_outcome
    return outcome
