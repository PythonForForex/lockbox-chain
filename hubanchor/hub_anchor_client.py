import asyncio
import json
import sys
import time
import uuid
from dataclasses import dataclass
from datetime import date, timedelta

import aiohttp

SUBNET = 'lockbox'
SEGMENT_KIND = 'state'
HUB_TRANSACTION_GAS = '400000'
READ_TIMEOUT_SECONDS = 10
RECONNECT_AFTER_SECONDS = 5
ANCHOR_COMMIT_WAIT = '30s'
NEW_BLOCK_HEADER_QUERY = "tm.event='NewBlockHeader'"


@dataclass(frozen=True)
class HubAnchorConfig:
    lockbox_rpc_url: str
    hub_node_url: str
    hub_rest_url: str
    hub_chain_id: str
    provider_id: str
    hub_key_name: str
    hub_home: str
    traderswiftd_path: str
    hub_keyring_backend: str = 'test'


def load_hub_anchor_config(config_path: str) -> HubAnchorConfig:
    with open(config_path) as config_file:
        return HubAnchorConfig(**json.load(config_file))


def log_event(event: str, correlation_id: str, **fields) -> None:
    record = {'event_id': uuid.uuid4().hex, 'ts': str(int(time.time() * 1000)), 'source': 'lockbox-hub-anchor',
              'correlation_id': correlation_id, 'event': event, **{name: str(value) for name, value in fields.items()}}
    print(json.dumps(record), file=sys.stdout, flush=True)


def utc_day_of(header_time: str) -> str:
    return header_time[:10]


def day_before(day: str) -> str:
    return (date.fromisoformat(day) - timedelta(days=1)).isoformat()


async def lockbox_rpc(http_session: aiohttp.ClientSession, config: HubAnchorConfig, path: str) -> dict:
    timeout = aiohttp.ClientTimeout(total=READ_TIMEOUT_SECONDS)
    async with http_session.get(config.lockbox_rpc_url + path, timeout=timeout) as response:
        return (await response.json())['result']


async def lockbox_header(http_session: aiohttp.ClientSession, config: HubAnchorConfig, height: int) -> dict:
    return (await lockbox_rpc(http_session, config, f'/header?height={height}'))['header']


async def last_height_of_day(http_session: aiohttp.ClientSession, config: HubAnchorConfig, day: str) -> int | None:
    sync_info = (await lockbox_rpc(http_session, config, '/status'))['sync_info']
    lowest, highest = int(sync_info['earliest_block_height']), int(sync_info['latest_block_height'])
    if utc_day_of(sync_info['latest_block_time']) <= day:
        return None
    if utc_day_of((await lockbox_header(http_session, config, lowest))['time']) > day:
        return None
    while lowest < highest:
        middle = (lowest + highest + 1) // 2
        if utc_day_of((await lockbox_header(http_session, config, middle))['time']) <= day:
            lowest = middle
        else:
            highest = middle - 1
    found_day = utc_day_of((await lockbox_header(http_session, config, lowest))['time'])
    return lowest if found_day == day else None


async def state_segment_of_day(http_session: aiohttp.ClientSession, config: HubAnchorConfig, day: str) -> tuple[str, str] | None:
    last_height = await last_height_of_day(http_session, config, day)
    if last_height is None:
        return None
    next_header = await lockbox_header(http_session, config, last_height + 1)
    return f'{day}:{last_height}', next_header['app_hash'].lower()


async def anchor_on_hub(http_session: aiohttp.ClientSession, config: HubAnchorConfig, segment_id: str) -> dict | None:
    timeout = aiohttp.ClientTimeout(total=READ_TIMEOUT_SECONDS)
    path = f'/network/v1/anchor/{SUBNET}/{SEGMENT_KIND}/{segment_id}/{config.provider_id}'
    async with http_session.get(config.hub_rest_url + path, timeout=timeout) as response:
        return await response.json() if response.status == 200 else None


async def run_traderswiftd(config: HubAnchorConfig, *arguments: str) -> str:
    process = await asyncio.create_subprocess_exec(config.traderswiftd_path, *arguments, stdin=asyncio.subprocess.DEVNULL,
                                                   stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    standard_output, standard_error = await process.communicate()
    return standard_output.decode() or standard_error.decode()


def first_json_object(text: str) -> dict:
    for line in text.splitlines():
        if line.startswith('{'):
            return json.loads(line)
    return {'code': -1, 'raw_log': text.strip()[-400:]}


async def broadcast_anchor(config: HubAnchorConfig, segment_id: str, segment_hash: str) -> dict:
    output = await run_traderswiftd(
        config, 'tx', 'network', 'anchor', '--subnet', SUBNET, '--segment-kind', SEGMENT_KIND, '--segment-id', segment_id,
        '--segment-hash', segment_hash, '--provider-id', config.provider_id, '--from', config.hub_key_name,
        '--home', config.hub_home, '--keyring-backend', config.hub_keyring_backend, '--chain-id', config.hub_chain_id,
        '--node', config.hub_node_url, '--gas', HUB_TRANSACTION_GAS, '--yes', '--output', 'json')
    return first_json_object(output)


async def log_when_committed(config: HubAnchorConfig, segment_id: str, transaction_hash: str) -> None:
    output = await run_traderswiftd(config, 'query', 'wait-tx', transaction_hash, '--node', config.hub_node_url,
                                    '--timeout', ANCHOR_COMMIT_WAIT, '--output', 'json')
    result = first_json_object(output)
    event = 'anchor_committed' if result.get('code', 0) == 0 and 'txhash' in result else 'anchor_not_committed'
    log_event(event, segment_id, transaction_hash=transaction_hash, code=result.get('code', ''), raw_log=result.get('raw_log', ''))


async def anchor_day(http_session: aiohttp.ClientSession, config: HubAnchorConfig, day: str, background_tasks: set) -> None:
    segment = await state_segment_of_day(http_session, config, day)
    if segment is None:
        log_event('day_has_no_closed_segment', day, day=day)
        return
    segment_id, segment_hash = segment
    stored = await anchor_on_hub(http_session, config, segment_id)
    if stored is not None:
        same = stored.get('segment_hash') == segment_hash
        log_event('anchor_already_on_hub' if same else 'anchor_on_hub_differs', segment_id, segment_hash=segment_hash,
                  hub_segment_hash=stored.get('segment_hash'))
        return
    outcome = await broadcast_anchor(config, segment_id, segment_hash)
    log_event('anchor_sent' if outcome.get('code') == 0 else 'anchor_refused', segment_id, segment_hash=segment_hash,
              transaction_hash=outcome.get('txhash', ''), code=outcome.get('code'), raw_log=outcome.get('raw_log', ''))
    if outcome.get('code') == 0:
        task = asyncio.create_task(log_when_committed(config, segment_id, outcome['txhash']))
        background_tasks.add(task)
        task.add_done_callback(background_tasks.discard)


async def anchor_day_safely(http_session: aiohttp.ClientSession, config: HubAnchorConfig, day: str, background_tasks: set) -> None:
    try:
        await anchor_day(http_session, config, day, background_tasks)
    except (aiohttp.ClientError, TimeoutError, KeyError, ValueError) as error:
        log_event('anchor_day_failed', day, day=day, error=repr(error))


def websocket_url_of(rpc_url: str) -> str:
    return rpc_url.replace('https://', 'wss://').replace('http://', 'ws://') + '/websocket'


async def follow_lockbox_headers(http_session: aiohttp.ClientSession, config: HubAnchorConfig, background_tasks: set) -> None:
    async with http_session.ws_connect(websocket_url_of(config.lockbox_rpc_url), heartbeat=30) as websocket:
        await websocket.send_json({'jsonrpc': '2.0', 'method': 'subscribe', 'id': 1, 'params': {'query': NEW_BLOCK_HEADER_QUERY}})
        current_day = utc_day_of((await lockbox_rpc(http_session, config, '/status'))['sync_info']['latest_block_time'])
        log_event('following_lockbox_headers', current_day, provider_id=config.provider_id)
        await anchor_day_safely(http_session, config, day_before(current_day), background_tasks)
        async for message in websocket:
            if message.type != aiohttp.WSMsgType.TEXT:
                break
            header = json.loads(message.data).get('result', {}).get('data', {}).get('value', {}).get('header')
            if header is None or utc_day_of(header['time']) == current_day:
                continue
            current_day = utc_day_of(header['time'])
            await anchor_day_safely(http_session, config, day_before(current_day), background_tasks)


async def run_hub_anchor_client(config: HubAnchorConfig) -> None:
    background_tasks: set = set()
    async with aiohttp.ClientSession() as http_session:
        while True:
            try:
                await follow_lockbox_headers(http_session, config, background_tasks)
            except (aiohttp.ClientError, TimeoutError, KeyError, ValueError) as error:
                log_event('lockbox_connection_lost', config.provider_id, error=repr(error))
            await asyncio.sleep(RECONNECT_AFTER_SECONDS)


def main() -> None:
    asyncio.run(run_hub_anchor_client(load_hub_anchor_config(sys.argv[1])))


if __name__ == '__main__':
    main()
