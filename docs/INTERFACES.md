# Interfaces between chain, signer and coordinator

All numbers that are prices or sizes are decimal strings. All hashes and keys are lowercase hex without `0x`, except Ethereum addresses, which are `0x` lowercase.

## Chain state (module `lockbox`)

| Item | Fields |
|---|---|
| RegisteredSigner | `signer_id`, `signer_address` |
| Account | `account_id`, `registration_key`, `owner_address` (empty until claimed) |
| KeyRef | `key_ref`, `account_id`, `venue_address`, `committee` (signer ids), `threshold` (at least 2) |
| Policy | `key_ref`, `delegate_key`, `epoch`, `venues` (not empty), `vault_address` (empty or `0x` lowercase), `asset_caps` (not empty), `expires_at` (unix seconds), `revoked`, `revoked_at` (block time, unix seconds; the first revoke holds) |
| AssetCap | `asset`, `max_order_size`, `max_daily_size` |
| AllowedRelease | `binary_hash`, `activate_height` |
| LogEntry | `height`, `request_id`, `key_ref`, `signer_id`, `decision` (`signed` or `refused`), `reason`, `intent_hash` |
| TapeAnchor | `segment_id`, `segment_hash` |

**Caps are sizes, per asset, per delegate, per UTC day.** There are no notional caps and no price band: a signer has no market price, and a notional cap is beaten by a low limit price. An asset not listed cannot be traded, reduce-only included. A sell at a low limit within the size caps is still signed; the caps bound how much, not at what price. `max_order_size` may not exceed `max_daily_size`.

**Daily size counts signed size, not fills, and failed rounds count.** It is per delegate, so two delegates each get the full cap. The owner-signed text says so. Reduce-only orders have no size caps and are not counted.

**Decimal format for caps:** no leading zeros, at most 16 digits before the point and 12 after, above zero. The chain refuses anything else, so a policy the signer could not read cannot be stored.

**One key_ref per sub-account.** The vault is pinned in the policy and repeated in the intent. `SetPolicy` refuses a vault that differs from another policy on the same key_ref, and a signer refuses an intent whose vault differs from its policy.

`epoch`: one counter per key_ref. Every `SetPolicy` and `RevokePolicy` must carry counter plus one. A Policy stores the counter value at its own last change, so with two delegates one policy's epoch can jump by more than one. The next value is `policy_epoch` on the keyref response; an intent carries the policy's own `epoch`.

## Messages

Exact texts the owner wallet signs are in `chain/README.md`. Fees are zero.

| Message | Who sends it | Proof inside |
|---|---|---|
| `RegisterSigner` | validator-set admin only | |
| `RegisterAccount` | any existing chain account | |
| `ClaimOwner` | any existing chain account | two signatures over one SIWE-form text: owner wallet (EIP-191) and registration key (ed25519) |
| `RegisterKeyRef` | any existing chain account | owner wallet signature; committee members must be registered signers; threshold at least 2 |
| `SetPolicy`, `RevokePolicy` | any existing chain account | owner wallet signature with the next epoch. Empty `venues` or empty `asset_caps` refused |
| `AllowRelease` | validator-set admin | takes effect after a delay in blocks |
| `RecordDecision` | the registered signer's own chain address, matching `signer_id` | |
| `AnchorTape` | a registered signer or the validator-set admin | |

`chain/policylogic` is policy validation and the owner texts, in one Go package with no SDK imports. **The signer is the one authority on intents;** the chain does not evaluate them.

## Reads

REST and CometBFT are different ports on the same node (`chain/README.md` has the numbers).

- REST: `GET /lockbox/v1/policy/{key_ref}/{delegate_key}`; `GET /lockbox/v1/keyref/{key_ref}`, whose response includes `policy_epoch`; `GET /lockbox/v1/account/{account_id}` → `{account_id, registration_key, owner_address}` (owner empty until `ClaimOwner`).
- CometBFT RPC: `GET /status` for height and latest block time.

## Order intent (JSON, delegate-signed)

```json
{"request_id":"uuid","key_ref":"...","delegate_key":"...","policy_epoch":1,"venue":"hyperliquid","vault_address":"",
 "kind":"order","asset_index":0,"asset":"BTC","is_buy":true,"limit_px":"100000","size":"0.001","reduce_only":false,
 "tif":"Alo","venue_nonce":1794000000000,"expires_at":1794000030,"delegate_signature":"..."}
```

`kind` is `order`, `cancel` (with `asset_index`, `oid`) or `schedule_cancel` (with `time_ms`). `vault_address` is required: empty, or a lowercase `0x` address. `delegate_key` is ed25519.

**Canonical bytes:** the intent with `delegate_signature` removed, keys sorted, no whitespace, strings printable ASCII only (anything else is refused, because Go, Python and Rust JSON encoders escape it differently and the signature covers the encoded bytes). Duplicate keys, unknown fields and floats are refused.

## Venue digest

Same bytes as `action_hash` in `hl-signer/lib/hl_signing.py`:

`msgpack(action) + nonce (8 bytes big-endian) + (0x00 | 0x01 + 20-byte vault) + 0x00 + expiresAfter (8 bytes big-endian)`

`expiresAfter` is `expires_at` times 1000, milliseconds per the venue docs. `vaultAddress` and `expiresAfter` go in the posted body as well, and the venue recomputes this hash from them. So `vault_address` and `expires_at` both bind at the venue.

## Rules every signer enforces, each on its own

1. **It never accepts a hash.** It builds the Hyperliquid action and digest itself from the intent.
2. **Full interactive protocol, no presignatures** (CVE-2025-66017 class).
3. **In this order**, from its own local node:
   - delegate signature valid; intent not expired; `expires_at` at most 120 s ahead (`intent_expiry_too_far`);
   - venue allowed; `vault_address` equals the policy's (`vault_mismatch`);
   - **close-out deadline** is the earlier of `revoked_at` and `expires_at`, plus 600 s (for a live policy, `expires_at` plus 600 s). At or after it, everything from this delegate is refused (`policy_closed`);
   - `cancel`: signed. `schedule_cancel`: `time_ms` present, 15 s to 600 s ahead, and at or before the deadline (`schedule_cancel_past_close_out_deadline`), then signed. Neither reads revoked, expired or epoch, so a cancel at the old epoch still signs after a revoke. An empty `time_ms` would remove the dead-man, so it is refused;
   - `order`, reduce-only included: not revoked, not expired, epoch equal to the policy's;
   - asset name maps to `asset_index` in the signer's own `asset_index_by_name` table (`asset_index_does_not_match_asset`). Always runs for an order; without it a delegate could name an allowed asset and send the index of another;
   - asset in `asset_caps` (`asset_not_allowed`). A reduce-only order stops here and is signed, on a stale chain too, using the last policy seen;
   - not stale (rule 4); `size` at most `max_order_size`; today's size for this delegate and asset plus `size` at most `max_daily_size`.
4. **Stale chain:** local node unreachable or catching up, behind by more than `max_blocks_behind`, or latest block older than `halt_timeout_seconds` (default 180): only `cancel`, `schedule_cancel` and reduce-only orders are signed. A dead node falls back to the last policy it saw; a live node that does not know the policy is a refusal.
5. **Nonce record:** the venue nonce must exceed the highest signed for that key_ref, and lie within (now minus 2 days, now plus `venue_nonce_max_ahead_milliseconds`, default 300 s). Written and synced to disk before the signer releases its signing message. A nonce written is used up even if the round fails.
6. **Catch-up quorum:** a signer cannot tell a restart from a restored backup, so after any start each key refuses everything, cancels included, until `threshold - 1` peers have answered with their record. A peer's claim beyond the window is ignored, so one bad peer cannot jam the key.
7. Every request, signed or refused, goes to `RecordDecision`. **Three-way log of a normal order:** with 2-of-3, two lines `signed` and one `refused` with reason `quorum_met_without_this_signer`, from the signer not chosen. A request refused by the asked signer before it proposes logs one line.

## Decision log transport

Each decision is written to the signer's disk outbox with fsync, then one worker wakes. One decision is one Cosmos transaction: `MsgRecordDecision` signed by the signer's own chain key (`SIGN_MODE_DIRECT`, secp256k1), sent to `POST /cosmos/tx/v1beta1/txs` with `BROADCAST_MODE_SYNC`. The order path never waits for this. An entry stays on disk until the account's sequence has passed it, and sending the same bytes again is harmless. There is no retry timer: what could not be delivered waits for the next decision or the next start. A request whose JSON cannot name itself is logged under `request_id` and `key_ref` `unparsed`.

## Signer API (HTTP on loopback; mutual TLS is required before two machines)

- `POST /dkg` `{key_ref, committee, threshold}` returns `{venue_address}` when the ceremony ends.
- `POST /sign` `{intent}` returns `{request_id, signed: true, action, digest, r, s, v}` or `{signed: false, reason}`. The coordinator asks one signer; signers talk to each other to complete the round.
- `GET /health`.
- `POST /import-share` `{public_data, sealed_share}` returns `{installed: true, key_ref, signer_id, venue_address}` or `{installed: false, reason}` (HTTP 409). Not on the loopback API: a separate listener (`import_share_listener` in the signer config) with the signer's own TLS 1.3, its TLS certificate and key, and exactly one pinned client certificate, the importer's (byte for byte, no CA). Any other certificate, or none, fails the handshake. Accepted only when: the share is addressed to this signer; the key ref is on the chain with the same `venue_address`, committee and threshold; this signer holds no share for it; the share opens with this signer's TLS private key; and the opened share matches the public data (its index is this signer's position in the sorted committee, size, threshold, shared public key, every public share, address). Stored through `key_share_store` exactly as a DKG share; signing does not change.

## Imported keys (dealer output)

`lockbox-key-import-dealer` (`dealer/`, run once on the importer's machine) reads the secret key as hex on stdin, refuses unless its address equals `--expected-address`, splits it with the library's trusted dealer (t-of-n, Shamir at points 1..n, fresh Paillier primes per signer) and writes, in a new directory:

| File | Fields |
|---|---|
| `public_data.json` | `format` (`lockbox-imported-key-public-data-v1`), `key_ref`, `committee` (sorted), `threshold`, `venue_address`, `shared_public_key` (compressed hex), `public_shares` (compressed hex, committee order), `recipient_certificate_sha256` (signer id to SHA-256 of its TLS certificate DER) |
| `share-<signer_id>.json` | `format` (`lockbox-imported-key-sealed-share-v1`), `key_ref`, `signer_id`, `recipient_certificate_sha256`, `public_data_sha256` (SHA-256 of `public_data` as serialized), `sealed` `{ephemeral_public_key, nonce, ciphertext}` |

Sealing: ECIES to the P-256 public key in the signer's pinned TLS certificate. Ephemeral P-256 key, ECDH, HKDF-SHA256 (salt: ephemeral public key then recipient public key, both compressed; info `lockbox-imported-share-sealing-v1`), AES-256-GCM with a random 12-byte nonce. Associated data binds the format, `key_ref`, `signer_id`, the certificate digest and `public_data_sha256`, so a share relabelled for another signer or paired with other public data does not open. The plaintext is the library's key share JSON.

Result event the import job writes after posting, on the operator's event stream: `event_id`, `ts`, `source`, `correlation_id`, `key_ref`, `venue_address`, `outcome` (`installed` only when all three signers answered `installed` with that address, else `refused`). Kit step 4 waits on it.

## Coordinator

Python, one per key, HTTP on loopback. The delegate signature covers `venue_nonce`, and the design says a coordinator cannot forge an intent, so it never holds the delegate key. A round therefore has two steps:

- `POST /prepare` `{delegate_key, vault_address}` returns `{key_ref, policy_epoch, venue_nonce, expires_at, rearm_intent}`. `rearm_intent` is null unless the dead-man needs re-arming; when present it is an unsigned `schedule_cancel` intent with its own nonce, its time limited to the policy's close-out deadline.
- `POST /intent` `{intent, rearm_intent}`, both signed by the delegate. Returns `{request_id, signed, digest, r, s, v, action, recovered, venue_status, venue_answer, rearm}` or `{signed: false, reason}`.

**The client checks before it signs** (`coordinator/delegate_client.py`). `rearm_intent` must be `schedule_cancel` only, with exactly the expected fields, its own key_ref, delegate and vault, `time_ms` 15 to 600 s ahead and expiry within 120 s. Anything else is refused: nothing is signed or sent, and the reason goes back to the caller. Two prepares can arrive out of order, which uses up the earlier nonce. On `nonce_not_above_highest_signed_` the client prepares once more and retries, and the result carries `retried_after` and `retry_of`. A second refusal returns its reason. The client takes the delegate signing function as an argument and holds no key itself.

It owns the venue nonce: rising, never below the current millisecond, and raised past any `nonce_not_above_highest_signed_N` a signer answers. Rounds on a key run one at a time. It asks signers in order and moves to the next only on a dead connection or a refusal local to that signer (`chain_stale_new_risk_stopped`, `catching_up_`, `policy_unavailable`, `key_ref_unavailable`); a policy refusal is final. It rebuilds the action and digest itself, and refuses any signature that does not recover to the chain's `venue_address`.

**Dead-man.** `scheduleCancel` is re-armed on a signing round when under half of `dead_man_window_seconds` (30 to 600, default 300) is left, per vault. There is no timer and no client heartbeat: if rounds stop, the book clears at the deadline, including for a strategy that is merely quiet. New risk is not sent if no deadline is in force; cancels and reduce-only orders go regardless. The venue caps triggers at 10 per UTC day and needs the time at least 5 s ahead. The venue refuses arming until the account has traded $1M ("Cannot set scheduled cancel time until enough volume traded."). On exactly that refusal, new risk still goes if the policy's largest per-asset daily cap, valued at the order's own limit price, stays under $10,000. The rule: the dead-man is required only above $10k. An order without a limit price is refused. Each such order logs `dead_man_unavailable_volume`. Any other refusal stops new risk. Settled 9 Nov.

The venue poster is `coordinator/venue_poster.py`. It refuses any URL that is not plain HTTP on loopback, except exactly `https://api.hyperliquid.xyz/exchange` when the config sets `allow_live_hyperliquid_exchange` to true. That setting is off by default and only the operator turns it on.

## Pending decisions

The `cggmp21` upgrade is not scheduled yet.

## Settled

- A restored or restarted signer keeps the catch-up for cancels too (rule 6). With one live peer the first cancel catches up and signs. This overrides the line in section 3 of the design that a signer never refuses a cancel for having missed signatures.
- Revoked or expired: bounded close-out, not open-ended (rule 3).

## Venue-connected phase

Not run yet. These need the real venue.

- **11th `scheduleCancel` firing in a UTC day.** The venue docs say the maximum is 10 triggers per day and stop there. Live test on a small account: let the dead-man fire 11 times in one UTC day and record what the venue does on the 11th: whether arming is refused, whether the cancel still runs, and what it answers. Until then, do not rely on the dead-man past the 10th firing of a day.
