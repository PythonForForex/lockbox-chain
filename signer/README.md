# lockbox-signer

One Rust daemon per signer. Distributed key generation for secp256k1, 2-of-3 signing by the full interactive protocol, the rules in `docs/INTERFACES.md`. Status: runs locally against the real chain, with real REST reads and `RecordDecision` transactions. Plain HTTP, so it refuses any non-loopback address. No exchange, VM or cloud is connected.

## Build and run

GMP is built from source by `gmp-mpfr-sys`, which needs `m4` on the PATH (a user-local build is fine). No `libgmp-dev`, no root.

```
cargo build --release
lockbox-signer signer-config.json
```

Config fields are in `src/signer_config.rs`. `identity_private_key_file` holds the signer's ed25519 peer identity as hex; each peer's public key is in `peers`. `asset_index_by_name` is the signer's own table of Hyperliquid asset name to index. `chain_key_file` holds the signer's secp256k1 chain key as hex; `chain_account_address` is its bech32 address, as registered by `RegisterSigner`, and a mismatch shows up as the chain refusing the transaction.

## Files

| File | What |
|---|---|
| `threshold_protocol.rs` | The only file that calls the library: keygen, auxiliary generation, interactive signing, low-s, recovery id |
| `hyperliquid_action.rs` | Builds the action, its msgpack and the phantom-agent digest from an intent. Follows Hyperliquid's published signing scheme |
| `order_intent.rs` | Strict JSON, canonical bytes, delegate signature, decimal parsing |
| `policy_reader.rs` | The only file that knows the chain's read shapes |
| `decision_recorder.rs` | Durable outbox and the one worker that drains it to the chain. Never blocks the order path |
| `cosmos_transaction.rs` | `MsgRecordDecision` as protobuf bytes, `SIGN_MODE_DIRECT`, signed with the chain key |
| `policy_rules.rs` | The rules, pure. The one authority on intents; the chain does not evaluate them |
| `nonce_record.rs` | Highest nonce and daily size per delegate and asset, per key, fsync before use, merge from peers |
| `chain_view.rs` | Policy, key ref and freshness for one request; last-seen cache for a dead node; catch up from peers |
| `signing_flow.rs` | `/sign` on the signer asked |
| `peer_signing_handlers.rs` | The same request on the other signers: proposal, begin, abort |
| `signing_round.rs` | One round on a worker thread, deadline, naming the peer at fault |
| `key_generation_flow.rs` | `/dkg`: prepare, start, compare addresses, commit |
| `import_share_listener.rs` | The `/import-share` listener: its own TLS 1.3, exactly one pinned client certificate (the importer's) |
| `key_import_flow.rs` | `/import-share`: key ref on chain, open the sealed share with the TLS key, check it against the public data, store it like a DKG share |
| `round_delivery.rs` | Connects the library's message stream to peers |
| `peer_transport.rs` | Signed envelopes between signers |
| `key_share_store.rs` | Share files. A committed share is never overwritten |
| `http_api.rs`, `main.rs`, `signer_state.rs`, `signer_config.rs` | The daemon |

## One signing round

1. The asked signer checks everything read-only, then sends the intent to every other committee member.
2. Each peer checks on its own, builds its own action and digest, writes its nonce record with fsync, and answers with its digest. A peer that restarted catches up first.
3. When `threshold - 1` peers accepted with an equal digest, the asked signer re-checks and writes its own record. It writes after the quorum because a catching-up peer reads its record: with the nonce already in it, the peer would refuse the same request as a repeat.
4. Chosen peers get `sign_begin`; a signer not chosen logs `quorum_met_without_this_signer`. Every chosen signer runs the protocol over its own digest. No digest ever crosses the wire as an input.
5. Each signer logs its decision. The asked signer answers with action, digest, r, s, v.

A nonce written in step 2 or 3 is used up even if the round fails. Retry with a higher one.

Any 2 of 3 signers overlap in one signer, and each proposal goes to everyone. So a repeated nonce or a blown daily size cap hits at least one signer that knows. The same holds for 3-of-5, not for 2-of-5.

## Restart

A signer cannot tell a clean restart from a restored old backup, so after any start each key is treated as restored. On the first request for a key it asks the committee for its record and needs `threshold - 1` answers. Until then it refuses everything, cancels included. A claimed nonce beyond the allowed window is ignored, so one bad peer cannot jam the key.

## Library

`cggmp21 = 0.6.3` pinned exact, with `Cargo.lock` committed. No `spof` feature, so `reconstruct_secret_key` and `trusted_dealer` do not compile into this binary. Imported keys are split by `../dealer/`, a separate crate that has `spof`; the signer uses that crate with its default features off, so it gets only the share opening and checks. No `hd-wallet`. `generate_presignature` is never called. See `docs/INTERFACES.md` rule 2.

**The maintainers' changelog for 0.6.3 warns that its zero-knowledge proofs still lack checks added in the CGGMP24 paper, and says to upgrade for complete protection.** Upstream is `cggmp24`, still alpha on crates.io. 0.6.3 is in the affected range of CVE-2025-66017 / GHSA-8frv-q972-9rq5, whose forgery needs presignatures, which this daemon never uses. The Kudelski report in the repo covers commit `d2b8494` of May 2023, 389 commits before 0.6.3. Pinned for throwaway keys only; the upgrade touches `threshold_protocol.rs` and `Cargo.toml`, and regenerates auxiliary data.

Reused: no code. From Guardian Wallet `packages/mpc-wasm/src/sign.rs` (Apache-2.0, commit `376a4c2`) the design point that `normalize_s` runs on the output and that a share never leaves the driver. That module drives `cggmp24` through a WASM state machine; this one uses the async API on `cggmp21` directly. TKeeper `docs/security-model/security-assurance.md` (commit `91ca186b`) was the acceptance list: duplicate JSON keys, intent mutation, eight-way nonce race, replay, in-flight crash, invalid proof with the peer named, transcript mutation, peer impersonation.

## Known limits

- Plain HTTP, loopback only. The library sends DKG secret shares over the transport in the clear, so mutual TLS is required before two machines.
- Share files are plain JSON, mode 0600. Encryption at rest is not built.
- An abort with no party named by the library is blamed on the counterparty, which is exact with two signers and `unknown` with more.
- No share refresh: this library version has no threshold refresh.
- A decision that could not be delivered waits on disk for the next decision or the next start. There is no retry timer.
- One decision is one transaction, so a normal order costs three transactions and a re-armed one six.
