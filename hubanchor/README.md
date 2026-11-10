# Hub anchor client

One small program per Lockbox validator. Once a UTC day it publishes Lockbox's state hash to the TraderSwift hub chain (`traderswift-1`) as `MsgAnchor(lockbox, state, <day>:<H>, <app hash>)`. The rules are in the Lockbox charter, version 1, on the hub.

## What it anchors

- `H` is the last Lockbox height whose header time falls in that UTC day.
- The hash is the `app_hash` in the header of `H + 1`, in lowercase hex. It is the state after block `H` ran.
- Every honest validator computes the same pair from its own node. The day is agreed when more than two thirds of the endorsed validator providers anchored the same hash, which is 3 of 4 today. The hub only records the anchors, and a reader compares them.

## How it runs

- It subscribes to `NewBlockHeader` on the validator's own Lockbox node. When a header carries a new UTC day, it anchors the day before. It does the same check each time it connects, so a restart catches up on yesterday.
- Before sending, it reads `/network/v1/anchor/lockbox/state/<segment_id>/<provider_id>` on the hub. If the anchor is already there, it sends nothing and logs whether the stored hash matches.
- It signs with `traderswiftd`, the hub's own binary, which is copied to the machine as a file. Lockbox's code imports nothing from the hub.
- It logs one JSON line per event to stdout. The events are `anchor_sent`, `anchor_committed`, `anchor_not_committed`, `anchor_already_on_hub`, `anchor_on_hub_differs`, `day_has_no_closed_segment`, `anchor_day_failed` and `lockbox_connection_lost`.
- Lockbox never waits on the hub. A hub outage costs that day's anchor and nothing else.

## Set up on a validator

1. Copy `traderswiftd` onto the machine. Make the validator's hub key there with `traderswiftd keys add hub-anchor --home /var/lib/lockbox-hub-anchor --keyring-backend test`. The key holds no money and is not a Lockbox key. It never leaves the machine.
2. The owner of the account that runs the validator signs `MsgRegisterProvider` for `subnet lockbox`, `provider_kind validator` and that key's address. An endorsed validator provider, or the operator, then endorses it.
3. Write the config and run it under systemd: `python3 -m hubanchor.hub_anchor_client /etc/lockbox/hub_anchor.json`, from the repo root.

```json
{"lockbox_rpc_url": "http://127.0.0.1:26657", "hub_node_url": "tcp://<hub node>:26657", "hub_rest_url": "http://<hub node>:1317",
 "hub_chain_id": "traderswift-1", "provider_id": "lockbox-validator-1", "hub_key_name": "hub-anchor",
 "hub_home": "/var/lib/lockbox-hub-anchor", "traderswiftd_path": "/usr/local/bin/traderswiftd"}
```
