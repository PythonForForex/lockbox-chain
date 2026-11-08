# Running a Lockbox validator

For an outside operator. You run one node that produces blocks on the Lockbox chain. Everything you need is in this repository. You can clone, configure, start and sync without contacting us. The only step that involves us is the last one, adding you to the validator set (section 7).

The network files are in [`networks/lockbox-devnet/`](../networks/lockbox-devnet/): chain ID, `genesis.json`, its SHA-256 and the peer list.

## What you get, and what you never get

| You get | You never get |
|---|---|
| A vote in block production, in proportion to the power the admin gives you | A key share, a signer role, or any part of a venue key |
| A full copy of the chain: accounts, policies, signer decisions, release hashes | The power to change policy, signers, releases or the validator set |
| Your node's own keys, made and kept on your machine | Any secret of ours, or any of our machines' keys |

Your node checks and orders transactions. It cannot sign orders; signers do that, under policies only an owner wallet can change. A validator alone cannot change any of that.

**The one-third rule.** While outside validators are few, the chain refuses any set where a single non-founding validator holds one third of the voting power or more. The founders cannot be outvoted by one outsider, and one outsider cannot halt the chain alone.

## 1. Machine

- Linux x86_64, 1 vCPU, 2 GB RAM, 20 GB disk is enough today. The binary is about 115 MB; a node used about 150 MB of memory after 10 minutes; the chain was 6.5 MB at height 499.
- A public IP, fixed if you can. Our nodes accept p2p on 26656 from anywhere.
- A clock kept by NTP.
- No other service on the machine needs our trust. Keep it to yourself.

## 2. The binary

Either build it from source:

```
git clone https://github.com/PythonForForex/lockbox-chain.git lockbox
cd lockbox/chain
go build -trimpath -o lockboxd .     # Go 1.26.5, as in chain/go.mod
sha256sum lockboxd
```

There is no published binary yet, so build from source.

Install it as `/opt/lockbox/bin/lockboxd` and run the node as its own user:

```
sudo useradd --system --home /var/lib/lockbox --create-home lockbox
sudo install -d -m 0755 /opt/lockbox/bin
sudo install -m 0755 lockboxd /opt/lockbox/bin/lockboxd
```

## 3. Your keys, made on your machine

```
sudo -u lockbox /opt/lockbox/bin/lockboxd init <your moniker> --chain-id lockbox-milestone-a-1 --home /var/lib/lockbox/chain
```

This writes two keys you keep:

| File | What | If lost or copied |
|---|---|---|
| `config/node_key.json` | p2p identity (your node ID) | Make a new one |
| `config/priv_validator_key.json` | consensus key that signs blocks | Copied: open an issue at once and we remove it. Never run two nodes with it: both would sign and the chain sees double signing |

Back up `priv_validator_key.json` offline. Never send it to anyone, us included.

You send us nothing yet. Section 7 says what to send once you are synced.

## 4. Genesis and peers

From the clone, check the hash, then copy genesis in:

```
cd lockbox/networks/lockbox-devnet
sha256sum -c genesis.sha256                     # must print: genesis.json: OK
sudo -u lockbox cp genesis.json /var/lib/lockbox/chain/config/genesis.json
paste -sd, peers.txt                            # your persistent_peers value
```

In `config/config.toml`:

```
[p2p]
laddr = "tcp://0.0.0.0:26656"
external_address = "tcp://<your public ip>:26656"
persistent_peers = "<the output of paste -sd, peers.txt>"
pex = false

[rpc]
laddr = "tcp://127.0.0.1:26657"
```

In `config/app.toml`: `minimum-gas-prices = "0ulockbox"`. Keep `[api]` and `[grpc]` on `127.0.0.1`.

## 5. Sync

**Block sync (default).** Start the node; it replays every block from genesis. The chain is small, so this is minutes.

**State sync (optional, not offered yet).** Only once we publish a read-only RPC address in `networks/lockbox-devnet/`. In `config.toml`:

```
[statesync]
enable = true
rpc_servers = "<rpc 1>,<rpc 2>"
trust_height = <height we publish>
trust_hash = "<block hash at that height>"
trust_period = "168h0m0s"
```

Check `trust_hash` yourself against a second source before you start. State sync needs our nodes to take snapshots (`state-sync.snapshot-interval` in their `app.toml`), which we turn on before we offer it.

## 6. Run under systemd

`/etc/systemd/system/lockbox-chain.service`:

```
[Unit]
Description=lockbox chain node
After=network-online.target
Wants=network-online.target

[Service]
User=lockbox
ExecStart=/opt/lockbox/bin/lockboxd start --home /var/lib/lockbox/chain
Restart=on-failure
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```
sudo systemctl daemon-reload && sudo systemctl enable --now lockbox-chain.service
curl -s http://127.0.0.1:26657/status | jq .result.sync_info
```

Wait until `catching_up` is `false`, and compare your height with a founding node's if you can. Until we add you, you are a full node: you follow the chain and sign nothing.

## 7. We add you

When you are synced, open an issue on https://github.com/PythonForForex/lockbox-chain/issues titled `validator: <your moniker>`, with the output of:

```
sudo -u lockbox /opt/lockbox/bin/lockboxd comet show-validator --home /var/lib/lockbox/chain   # consensus public key
sudo -u lockbox /opt/lockbox/bin/lockboxd comet show-node-id --home /var/lib/lockbox/chain     # node ID
curl -s http://127.0.0.1:26657/status | jq -r .result.sync_info.latest_block_height           # your height
```

plus your public IP. These are public keys and IDs only. Nothing here lets anyone act as you. Never paste `priv_validator_key.json` or `node_key.json`.

Our admin then sends one transaction. The admin is one address fixed in genesis, held by the founders. Today it is a single key, not a multisig. Moving it to a founders' multisig is planned and will be announced in this repository:

```
lockboxd tx validatorset add-validator \
  --consensus-public-key <the "key" value from show-validator> \
  --power <power> --moniker <your moniker> \
  --from <admin> --chain-id lockbox-milestone-a-1
```

`--founding` is never set for an outside validator. The chain refuses the add if it would give you one third of the power or more. The change reaches CometBFT at the end of that block and takes effect two blocks later. You see it as your address in `/validators`:

```
curl -s http://127.0.0.1:26657/validators | jq '.result.validators[] | {address, voting_power}'
lockboxd query validatorset validators --node tcp://127.0.0.1:26657
```

The admin can also change your power (`set-validator-power`) or remove you (`remove-validator`). Removal takes effect two blocks later; your node keeps running as a full node with its copy of the chain.

## 8. Firewall

Open only p2p:

```
table inet filter {
  chain input {
    type filter hook input priority 0; policy drop;
    iif lo accept
    ct state established,related accept
    tcp dport 22 ip saddr <your admin ip> accept
    tcp dport 26656 accept
  }
}
```

RPC (26657), REST (1317), gRPC (9090) and pprof stay on loopback. Use an SSH tunnel to read them from elsewhere.

## 9. Monitoring

| Watch | How | Alert when |
|---|---|---|
| Height moving | `curl -s 127.0.0.1:26657/status \| jq -r .result.sync_info.latest_block_height` | Unchanged for 30 s |
| Catching up | `.result.sync_info.catching_up` | `true` for more than a few minutes |
| Peers | `curl -s 127.0.0.1:26657/net_info \| jq .result.n_peers` | Fewer than 2 |
| Signing | your `address` in `/commit?height=<h>` signatures, `block_id_flag` 2 | Missing for 10 blocks in a row |
| Disk, memory | the usual | Disk above 80% |
| Process | `systemctl is-active lockbox-chain` | Not active |

CometBFT also serves Prometheus metrics if you set `prometheus = true` under `[instrumentation]` in `config.toml`; keep that port on loopback too.

## 10. Upgrades and leaving

We announce a new release in this repository with its hash and the height to switch at. Watch the repository, and `networks/lockbox-devnet/README.md` for a fresh genesis. Stop, swap the binary, start. To leave, open an issue. We remove you, then you stop the node. Keep or delete your copy of the chain as you like; it holds no secret.
