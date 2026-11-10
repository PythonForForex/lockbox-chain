# lockbox coordinator

One per key. Takes a delegate-signed intent, asks a signer, checks the answer, posts to the venue. Contract: `../docs/INTERFACES.md`.

**Live venue: off by default; the operator turns it on.** `venue_poster.py` refuses any URL but plain HTTP on loopback. The one exception is exactly `https://api.hyperliquid.xyz/exchange`, and only with `allow_live_hyperliquid_exchange: true` in the config. Only the operator sets it.

```
cd lockbox
python3 -m coordinator.coordinator_service coordinator-config.json
```

Config is the fields of `CoordinatorConfig`: `listen_host`, `listen_port`, `key_ref`, `signer_urls`, `chain_rest_urls`, `venue_exchange_url`, `dead_man_window_seconds` (30 to 600, default 300), `intent_lifetime_seconds` (at most 120, the signers refuse beyond that), `allow_live_hyperliquid_exchange` (default false), and for the hub owner check `hub_rest_urls`, `hub_websocket_url` and `hub_account_id` (all empty by default, which turns the check off). The check is in `hub_owner_check.py`, and the rule is in `docs/INTERFACES.md`.

| File | What |
|---|---|
| `coordinator_service.py` | HTTP routes, config, start |
| `coordinator_rounds.py` | Prepare, nonce, dead-man, one signing round and its checks |
| `signer_client.py` | Asks signers in order, moves on only for a dead connection or a local refusal |
| `chain_reader.py` | `venue_address` and policy epoch from REST |
| `venue_digest.py` | Own build of the action and digest, signature recovery. Follows Hyperliquid's published signing scheme |
| `venue_poster.py` | The one outbound call to the venue |
| `delegate_client.py` | What a bot imports: checks `rearm_intent` before signing, signs through a function it is given, re-prepares once on a stale nonce |

It holds no key. The delegate signs `venue_nonce` too, so a client calls `/prepare` for the nonce and any re-arm intent, signs, then calls `/intent`.
