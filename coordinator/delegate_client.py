import json
import time
import uuid
from collections.abc import Callable

import aiohttp

MILLISECONDS_PER_SECOND = 1000
MAXIMUM_INTENT_LIFETIME_SECONDS = 120
MINIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS = 15
MAXIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS = 600
SUPPORTED_VENUE = 'hyperliquid'
REQUEST_TIMEOUT_SECONDS = 120
STALE_NONCE_REFUSAL = 'nonce_not_above_highest_signed_'
REARM_INTENT_FIELDS = frozenset({
    'request_id', 'key_ref', 'delegate_key', 'policy_epoch', 'venue', 'vault_address', 'kind', 'time_ms', 'venue_nonce', 'expires_at'})


def canonical_intent_bytes(intent: dict) -> bytes:
    unsigned = {name: value for name, value in intent.items() if name != 'delegate_signature'}
    return json.dumps(unsigned, sort_keys=True, separators=(',', ':')).encode()


def sign_intent(intent: dict, sign_bytes: Callable[[bytes], str]) -> dict:
    return {**intent, 'delegate_signature': sign_bytes(canonical_intent_bytes(intent))}


def reason_to_refuse_rearm(rearm_intent: dict, key_ref: str, delegate_key: str, vault_address: str) -> str | None:
    if not isinstance(rearm_intent, dict) or rearm_intent.get('kind') != 'schedule_cancel':
        return 'not_schedule_cancel'
    if set(rearm_intent) != REARM_INTENT_FIELDS:
        return 'unexpected_fields'
    if rearm_intent['key_ref'] != key_ref:
        return 'wrong_key_ref'
    if rearm_intent['delegate_key'] != delegate_key:
        return 'wrong_delegate_key'
    if rearm_intent['vault_address'] != vault_address:
        return 'wrong_vault'
    if rearm_intent['venue'] != SUPPORTED_VENUE:
        return 'wrong_venue'
    now_milliseconds = int(time.time() * MILLISECONDS_PER_SECOND)
    time_ms = rearm_intent['time_ms']
    if not isinstance(time_ms, int) or isinstance(time_ms, bool):
        return 'time_missing'
    if time_ms < now_milliseconds + MINIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS * MILLISECONDS_PER_SECOND:
        return 'time_too_soon'
    if time_ms > now_milliseconds + MAXIMUM_SCHEDULE_CANCEL_AHEAD_SECONDS * MILLISECONDS_PER_SECOND:
        return 'time_too_far'
    if rearm_intent['expires_at'] > now_milliseconds // MILLISECONDS_PER_SECOND + MAXIMUM_INTENT_LIFETIME_SECONDS:
        return 'expiry_too_far'
    return None


async def post_json(http_session: aiohttp.ClientSession, url: str, body: dict) -> dict:
    timeout = aiohttp.ClientTimeout(total=REQUEST_TIMEOUT_SECONDS)
    async with http_session.post(url, json=body, timeout=timeout) as response:
        return await response.json()


async def prepare_round(http_session: aiohttp.ClientSession, coordinator_url: str, delegate_key: str, vault_address: str) -> dict:
    return await post_json(http_session, f'{coordinator_url}/prepare', {'delegate_key': delegate_key, 'vault_address': vault_address})


async def submit_prepared(http_session: aiohttp.ClientSession, coordinator_url: str, prepared: dict, key_ref: str, delegate_key: str,
                          vault_address: str, sign_bytes: Callable[[bytes], str], build_fields: Callable[[dict], dict]) -> dict:
    if not prepared.get('prepared'):
        return {'signed': False, 'reason': f"prepare_refused: {prepared.get('reason')}"}
    if prepared.get('key_ref') != key_ref:
        return {'signed': False, 'reason': 'client_refused_prepare: wrong_key_ref'}
    rearm_intent = prepared.get('rearm_intent')
    if rearm_intent is not None:
        refusal = reason_to_refuse_rearm(rearm_intent, key_ref, delegate_key, vault_address)
        if refusal:
            return {'signed': False, 'reason': f'client_refused_rearm_intent: {refusal}'}
    intent = {
        'request_id': uuid.uuid4().hex,
        'key_ref': key_ref,
        'delegate_key': delegate_key,
        'policy_epoch': prepared['policy_epoch'],
        'venue': SUPPORTED_VENUE,
        'vault_address': vault_address,
        'venue_nonce': prepared['venue_nonce'],
        'expires_at': prepared['expires_at'],
        **build_fields(prepared),
    }
    body = {'intent': sign_intent(intent, sign_bytes)}
    if rearm_intent is not None:
        body['rearm_intent'] = sign_intent(rearm_intent, sign_bytes)
    reply = await post_json(http_session, f'{coordinator_url}/intent', body)
    return {**reply, 'intent': intent}


# Two prepares can reach the signers out of order, and the earlier nonce is then used up. Nothing was signed for it, so
# one fresh prepare is safe. A second refusal goes back to the caller with its reason.
async def run_client_round(http_session: aiohttp.ClientSession, coordinator_url: str, key_ref: str, delegate_key: str, vault_address: str,
                           sign_bytes: Callable[[bytes], str], build_fields: Callable[[dict], dict], prepared: dict | None = None) -> dict:
    first_prepared = prepared or await prepare_round(http_session, coordinator_url, delegate_key, vault_address)
    outcome = await submit_prepared(http_session, coordinator_url, first_prepared, key_ref, delegate_key, vault_address, sign_bytes, build_fields)
    if outcome.get('signed') or STALE_NONCE_REFUSAL not in str(outcome.get('reason')):
        return outcome
    first_reason, first_request_id = outcome['reason'], outcome['intent']['request_id']
    fresh_prepared = await prepare_round(http_session, coordinator_url, delegate_key, vault_address)
    retried = await submit_prepared(http_session, coordinator_url, fresh_prepared, key_ref, delegate_key, vault_address, sign_bytes, build_fields)
    return {**retried, 'retried_after': first_reason, 'retry_of': first_request_id}
