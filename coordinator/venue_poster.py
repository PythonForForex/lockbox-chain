import json
from urllib.parse import urlsplit

import aiohttp

POST_TIMEOUT_SECONDS = 10
LOOPBACK_HOSTS = ('127.0.0.1', 'localhost', '::1')
LIVE_HYPERLIQUID_EXCHANGE_URL = 'https://api.hyperliquid.xyz/exchange'


def refuse_venue_url(venue_exchange_url: str, allow_live_hyperliquid_exchange: bool) -> None:
    if allow_live_hyperliquid_exchange is True and venue_exchange_url == LIVE_HYPERLIQUID_EXCHANGE_URL:
        return
    if urlsplit(venue_exchange_url).scheme == 'http' and urlsplit(venue_exchange_url).hostname in LOOPBACK_HOSTS:
        return
    raise ValueError(f'venue_exchange_url {venue_exchange_url} refused: loopback only, or exactly {LIVE_HYPERLIQUID_EXCHANGE_URL} '
                     'with allow_live_hyperliquid_exchange true')


def build_signed_body(action: dict, venue_nonce: int, r_hex: str, s_hex: str, v: int, vault_address: str, expires_after: int) -> dict:
    return {
        'action': action,
        'nonce': venue_nonce,
        'signature': {'r': hex(int(r_hex, 16)), 's': hex(int(s_hex, 16)), 'v': v},
        'vaultAddress': vault_address or None,
        'expiresAfter': expires_after,
    }


async def post_signed_action(http_session: aiohttp.ClientSession, venue_exchange_url: str, allow_live_hyperliquid_exchange: bool,
                             body: dict) -> tuple[str, dict]:
    refuse_venue_url(venue_exchange_url, allow_live_hyperliquid_exchange)
    timeout = aiohttp.ClientTimeout(total=POST_TIMEOUT_SECONDS)
    try:
        async with http_session.post(venue_exchange_url, json=body, timeout=timeout) as response:
            text = await response.text()
            status_code = response.status
    except (aiohttp.ClientError, TimeoutError) as cut:
        return 'transport_error', {'error': type(cut).__name__}
    if status_code >= 500:
        return 'transport_error', {'http_status': status_code}
    try:
        answer = json.loads(text)
    except ValueError:
        return 'transport_error', {'non_json_body': text[:300]}
    return ('ok' if answer.get('status') == 'ok' else 'err'), answer
