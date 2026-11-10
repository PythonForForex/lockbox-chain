import asyncio
import json

import aiohttp

from coordinator.chain_reader import read_first_answer

HUB_READ_TIMEOUT_SECONDS = 2
RECONNECT_AFTER_SECONDS = 5


def hub_owner_check_is_on(config) -> bool:
    return bool(config.hub_rest_urls and config.hub_account_id)


def build_owner_cache() -> dict:
    return {'lockbox_owner_address': None, 'hub_answer': None}


async def read_lockbox_owner_address(http_session: aiohttp.ClientSession, config) -> str | None:
    key_ref = await read_first_answer(http_session, config.chain_rest_urls, f'/lockbox/v1/keyref/{config.key_ref}')
    if key_ref is None:
        return None
    account = await read_first_answer(http_session, config.chain_rest_urls, f"/lockbox/v1/account/{key_ref['account_id']}")
    return account.get('owner_address') if account else None


async def read_hub_answer(http_session: aiohttp.ClientSession, config) -> dict | None:
    timeout = aiohttp.ClientTimeout(total=HUB_READ_TIMEOUT_SECONDS)
    for hub_rest_url in config.hub_rest_urls:
        try:
            async with http_session.get(f'{hub_rest_url}/network/v1/account/{config.hub_account_id}', timeout=timeout) as response:
                if response.status == 200:
                    return {'found': True, 'owner_address': (await response.json()).get('owner_address', '')}
                if response.status == 404:
                    return {'found': False, 'owner_address': ''}
        except (aiohttp.ClientError, TimeoutError):
            continue
    return None


async def refresh_owner_cache(state: dict, reason: str, log_event) -> None:
    config, http_session, cache = state['config'], state['http_session'], state['owner_cache']
    lockbox_owner_address = await read_lockbox_owner_address(http_session, config)
    if lockbox_owner_address is not None:
        cache['lockbox_owner_address'] = lockbox_owner_address
    hub_answer = await read_hub_answer(http_session, config)
    if hub_answer is not None:
        cache['hub_answer'] = hub_answer
    log_event('hub_owner_cache_refreshed', config.hub_account_id, reason=reason, hub_reachable=hub_answer is not None,
              hub_account_found=(cache['hub_answer'] or {}).get('found', ''), refusal=hub_owner_refusal(state) or '')


def hub_owner_refusal(state: dict) -> str | None:
    if not hub_owner_check_is_on(state['config']):
        return None
    cache = state['owner_cache']
    hub_answer, lockbox_owner_address = cache['hub_answer'], cache['lockbox_owner_address']
    if hub_answer is None or lockbox_owner_address is None:
        return None
    if not hub_answer['found']:
        return 'hub_account_missing'
    if not hub_answer['owner_address']:
        return 'hub_account_unclaimed'
    if hub_answer['owner_address'].lower() != lockbox_owner_address.lower():
        return 'hub_owner_differs'
    return None


def claimed_query(hub_account_id: str) -> str:
    return f"tm.event='Tx' AND network.claimed.account_id='{hub_account_id}'"


async def follow_hub_claims_once(state: dict, log_event) -> None:
    config = state['config']
    async with state['http_session'].ws_connect(config.hub_websocket_url, heartbeat=30) as websocket:
        await websocket.send_json({'jsonrpc': '2.0', 'method': 'subscribe', 'id': 1, 'params': {'query': claimed_query(config.hub_account_id)}})
        await refresh_owner_cache(state, 'hub_connected', log_event)
        async for message in websocket:
            if message.type != aiohttp.WSMsgType.TEXT:
                break
            if json.loads(message.data).get('result', {}).get('data'):
                await refresh_owner_cache(state, 'hub_claimed_event', log_event)


async def follow_hub_claims(state: dict, log_event) -> None:
    while True:
        try:
            await follow_hub_claims_once(state, log_event)
        except (aiohttp.ClientError, TimeoutError, ValueError) as error:
            log_event('hub_connection_lost', state['config'].hub_account_id, error=repr(error))
        await asyncio.sleep(RECONNECT_AFTER_SECONDS)
