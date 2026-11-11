# lockbox-devnet

The Lockbox development network. It runs in dev mode with test keys, and we are testing orders on Hyperliquid with it.

| | |
|---|---|
| Chain ID | `lockbox-milestone-a-1` |
| Genesis | `genesis.json`, SHA-256 in `genesis.sha256` |
| Genesis time | 2026-11-08T08:39:36Z |
| CometBFT | v0.40.0 in `chain/go.mod`; `/status` reports 0.39.0 (ABCI 2.0.0, block protocol 11, p2p protocol 8) |
| Peers | `peers.txt`, one `nodeid@ip:26656` per line, four founding validators |
| p2p | 26656 open to everyone |

`genesis.json` is copied from the running nodes. All four founding validators hold the same file.

## Changes to expect

- **Peers.** The list can change when we move a node. `peers.txt` is always current.
- **Retired.** `lockbox-milestone-a-1` was retired on 11 Nov 2026. Do not start a node for it. The project continues as Botsign.

To join, follow [docs/VALIDATOR_JOIN.md](../../docs/VALIDATOR_JOIN.md).

What this network has proven so far: [docs/MILESTONE_A_RESULTS.md](../../docs/MILESTONE_A_RESULTS.md).
