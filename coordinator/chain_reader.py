import aiohttp

READ_TIMEOUT_SECONDS = 2


async def read_first_answer(http_session: aiohttp.ClientSession, chain_rest_urls: tuple[str, ...], path: str) -> dict | None:
    timeout = aiohttp.ClientTimeout(total=READ_TIMEOUT_SECONDS)
    for chain_rest_url in chain_rest_urls:
        try:
            async with http_session.get(chain_rest_url + path, timeout=timeout) as response:
                if response.status == 200:
                    return await response.json()
        except (aiohttp.ClientError, TimeoutError):
            continue
    return None


async def read_venue_address(http_session: aiohttp.ClientSession, chain_rest_urls: tuple[str, ...], key_ref: str) -> str | None:
    document = await read_first_answer(http_session, chain_rest_urls, f'/lockbox/v1/keyref/{key_ref}')
    return document['venue_address'] if document else None


async def read_policy(http_session: aiohttp.ClientSession, chain_rest_urls: tuple[str, ...], key_ref: str, delegate_key: str) -> dict | None:
    return await read_first_answer(http_session, chain_rest_urls, f'/lockbox/v1/policy/{key_ref}/{delegate_key}')
