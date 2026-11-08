import aiohttp

SIGN_TIMEOUT_SECONDS = 40
REASONS_LOCAL_TO_THE_ASKED_SIGNER = (
    'chain_stale_new_risk_stopped',
    'catching_up_',
    'policy_unavailable',
    'key_ref_unavailable',
)


async def ask_signers(http_session: aiohttp.ClientSession, signer_urls: tuple[str, ...], intent: dict) -> dict:
    timeout = aiohttp.ClientTimeout(total=SIGN_TIMEOUT_SECONDS)
    last_reply = {'signed': False, 'reason': 'all_signers_unreachable'}
    for signer_url in signer_urls:
        try:
            async with http_session.post(f'{signer_url}/sign', json={'intent': intent}, timeout=timeout) as response:
                reply = await response.json()
        except TimeoutError:
            return {'signed': False, 'reason': 'sign_timeout_state_unknown', 'asked': signer_url}
        except aiohttp.ClientError:
            continue
        reply['asked'] = signer_url
        if reply.get('signed') or not reply.get('reason', '').startswith(REASONS_LOCAL_TO_THE_ASKED_SIGNER):
            return reply
        last_reply = reply
    return last_reply
