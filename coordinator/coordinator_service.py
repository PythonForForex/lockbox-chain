import asyncio
import json
import sys

import aiohttp
from aiohttp import web

from coordinator.coordinator_rounds import (MAXIMUM_DEAD_MAN_WINDOW_SECONDS, MINIMUM_DEAD_MAN_WINDOW_SECONDS, CoordinatorConfig,
                                            build_round_state, handle_intent, log_event, prepare_round)
from coordinator.hub_owner_check import follow_hub_claims, hub_owner_check_is_on
from coordinator.venue_poster import refuse_venue_url


def load_coordinator_config(config_path: str) -> CoordinatorConfig:
    with open(config_path) as config_file:
        fields = json.load(config_file)
    fields['signer_urls'] = tuple(fields['signer_urls'])
    fields['chain_rest_urls'] = tuple(fields['chain_rest_urls'])
    fields['hub_rest_urls'] = tuple(fields.get('hub_rest_urls', ()))
    config = CoordinatorConfig(**fields)
    refuse_venue_url(config.venue_exchange_url, config.allow_live_hyperliquid_exchange)
    if not MINIMUM_DEAD_MAN_WINDOW_SECONDS <= config.dead_man_window_seconds <= MAXIMUM_DEAD_MAN_WINDOW_SECONDS:
        raise ValueError(f'dead_man_window_seconds must be {MINIMUM_DEAD_MAN_WINDOW_SECONDS} to {MAXIMUM_DEAD_MAN_WINDOW_SECONDS}')
    return config


async def handle_health(request: web.Request) -> web.Response:
    state = request.app['round_state']
    return web.json_response({'status': 'ok', 'key_ref': state['config'].key_ref, 'venue_address': state['venue_address']})


async def handle_prepare(request: web.Request) -> web.Response:
    body = await request.json()
    async with request.app['round_lock']:
        reply = await prepare_round(request.app['round_state'], body['delegate_key'], body['vault_address'])
    return web.json_response(reply)


async def handle_intent_request(request: web.Request) -> web.Response:
    body = await request.json()
    async with request.app['round_lock']:
        reply = await handle_intent(request.app['round_state'], body['intent'], body.get('rearm_intent'))
    return web.json_response(reply)


async def run_service(config: CoordinatorConfig) -> None:
    async with aiohttp.ClientSession() as http_session:
        application = web.Application()
        application['round_state'] = build_round_state(config, http_session)
        application['round_lock'] = asyncio.Lock()
        hub_claims_task = None
        if hub_owner_check_is_on(config):
            hub_claims_task = asyncio.create_task(follow_hub_claims(application['round_state'], log_event))
        application.add_routes([
            web.get('/health', handle_health),
            web.post('/prepare', handle_prepare),
            web.post('/intent', handle_intent_request),
        ])
        runner = web.AppRunner(application, access_log=None)
        await runner.setup()
        await web.TCPSite(runner, config.listen_host, config.listen_port).start()
        await asyncio.Event().wait()


def main() -> None:
    asyncio.run(run_service(load_coordinator_config(sys.argv[1])))


if __name__ == '__main__':
    main()
