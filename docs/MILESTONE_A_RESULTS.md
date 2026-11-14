# Milestone A results

The Milestone A test is complete. The chain `lockbox-milestone-a-1` was retired on 11 Nov 2026. Lockbox continues as Signer, part of TraderSwift: https://github.com/traderswift-io

8 Nov 2026, chain `lockbox-milestone-a-1`. Everything below can be checked against the chain and the venue's public API. Nothing here asks you to trust us.

## What was proven

1. A live Hyperliquid trading key was held as 2-of-3 threshold shares on three signer machines. The key was never whole on any of them, and real orders were signed and filled with it.
2. A signer signs only what the policy on Lockbox's chain allows. The owner sets that policy, and the chain records every decision, both signed and refused, with its reason.
3. A withdrawal, a transfer, an order over the size limit, an order after the policy was revoked, an old nonce, and a round with too few signers were all refused. Each refusal is on the chain.
4. An external validator, on its own machine, joined the validator set and signed the blocks that recorded the last orders.
5. A node set up by a Grok bot became a validator and signed the blocks that recorded two live orders on 11 Nov, together with the external validator.

## Who ran what

| Part | How many | Run by | Where |
|---|---|---|---|
| Founding validators `lockbox-1` to `lockbox-4` | 4, power 10,000,000 each | Us | One rented cloud VM each, all with one provider |
| Signers `s1`, `s2`, `s3` | 3 | Us | `s1` on `lockbox-1`, `s2` on `lockbox-2`, `s3` on `lockbox-3`, next to the validators there |
| Coordinator | 1 | Us | On `lockbox-4` |
| The external validator | 1, power 1,000,000 | Not us | Its own machine; added at height 33254 |
| A node run by a Grok bot | 1 validator, power 1,000,000 | Not us | Its own machine, `grok-test-1`. Built from this repo and synced on its own, then added as a validator at height 150200 from its relayed key. It signed 12 blocks between 150204 and 150243 and went offline. Restarted by the bot on 11 Nov, it signed the blocks recording orders 601320050651 and 601320087888 |

## The evidence

Every order in this test was signed by signers we run ourselves. No outside party held a key share. The external validator signed blocks, not orders.

| Item | Value |
|---|---|
| Chain id | `lockbox-milestone-a-1` (genesis and peers in `networks/lockbox-devnet/`) |
| Key ref | `kr-milestone-a-3`, committee `s1, s2, s3`, threshold 2 |
| Venue key (agent address) | [`0xfce7a019fc4b20c2c646e393728e27f05cbfb06b`](https://app.hyperliquid.xyz/explorer/address/0xfce7a019fc4b20c2c646e393728e27f05cbfb06b) |
| Venue account it trades | [`0x97a7c73667d002ca31c8f045c11d017b1bc2c904`](https://app.hyperliquid.xyz/explorer/address/0x97a7c73667d002ca31c8f045c11d017b1bc2c904) (a Hyperliquid sub-account) |
| Policy | Set at height 26032: BTC only, at most 0.0003 BTC per order, at most 0.003 BTC a day, that account only |
| The external validator | Added at height 33254 with power 1,000,000; consensus address `7CF2142DA4D3061427E399FBFD3D5698482640D4` |

### Orders that filled

The venue order ids come from Hyperliquid's public order history for the account above. Each venue transaction link opens the signed order on Hyperliquid's explorer; the full hashes are in the transactions table below. Each order is one signed round, followed by a second round that re-arms the dead-man switch. The rounds are matched to orders by sequence and time. "Block signed by" is the set of validators in `/commit?height=<block>` for the block that recorded the signed round.

| Venue order | Venue transaction | Side | Venue time (UTC) | Signed rounds on the chain | Signers (server) | Block signed by |
|---|---|---|---|---|---|---|
| 598536092472 | [`0xa255e0af…`](https://app.hyperliquid.xyz/explorer/tx/0xa255e0afc0773b31a3cf04485b9e0902028a00955b7a5a03461e8c027f7b151c) | buy, IOC, filled | 17:20:16 | request `9ef85b293f774ab5992f1e64709bde6e`, block 26261 | s1 (lockbox-1), s3 (lockbox-3) | lockbox-1, -2, -3, -4 |
| 598536117844 | [`0xac63460b…`](https://app.hyperliquid.xyz/explorer/tx/0xac63460b1d918974addd04485b9e4202014f00f0b894a846502bf15ddc95635f) | reduce-only sell, filled | 17:20:20 | request `a31ed065d7324531b38ba5c352701a24`, block 26265 | s1 (lockbox-1), s2 (lockbox-2) | lockbox-1, -2, -3, -4 |
| 598603151430 | [`0xd1601a9b…`](https://app.hyperliquid.xyz/explorer/tx/0xd1601a9be087b9e0d2d904485d8c6802011800817b8ad8b27528c5ee9f8b93cb) | buy, IOC, filled | 19:49:04 | request `91d46675868e425b9931b08c77a28122`, block 33288 | s1 (lockbox-1), s3 (lockbox-3) | lockbox-1, -2, -3, -4 and the external validator |
| 598603170550 | [`0xa81eaf8c…`](https://app.hyperliquid.xyz/explorer/tx/0xa81eaf8cfc78c740a99804485d8c9d016600c772977be6124be75adfbb7ca12b) | reduce-only sell, filled | 19:49:08 | request `ddc2888e74e24430980d056fb4f89423`, block 33291 | s1 (lockbox-1), s3 (lockbox-3) | lockbox-1, -2, -3, -4 and the external validator |
| 601320050651 | [`0x43b710a5…`](https://app.hyperliquid.xyz/explorer/tx/0x43b710a5ede129b5453004488911eb02018f008b88e44887e77fbbf8ace5039f) | buy, IOC, filled | 04:23:56 (11 Nov) | request `9a0af42d6d984dd09581e90f169de414`, block 188408 | s1 (lockbox-1), s3 (lockbox-3) | lockbox-1, -2, -3, -4, the external validator and the Grok bot's node |
| 601320087888 | [`0xdf8607fd…`](https://app.hyperliquid.xyz/explorer/tx/0xdf8607fdef9df0d2e0ff04488912250202c000e38a910fa4834eb350ae91cabd) | reduce-only sell, filled | 04:24:00 (11 Nov) | request `03ecf43aaa4345b0821148c1f22ab6e2`, block 188412 | s1 (lockbox-1), s2 (lockbox-2) | lockbox-1, -2, -3, -4, the external validator and the Grok bot's node |

In each of the 8 Nov 19:49 rounds, s2 recorded `quorum_met_without_this_signer`: two signers were enough, so the third one stood down. In the 11 Nov rounds the third signer recorded a refusal too: s2 in three rounds, s3 in one. Blocks 33287 to 33293 carry the external validator's signature in their commits. Blocks 188407 to 188413, on 11 Nov, carry both the external validator's and the Grok bot's node's.

### Refusals, with the chain's own reason

| Request | Block | Signer (server) | Reason |
|---|---|---|---|
| `2da216371a1646ef93e3489818bb7767` | 26051 | s1 (lockbox-1) | a withdrawal with a destination: `bad_intent_json: unknown field destination` |
| `52b0b57968e7443b8e6b131adbaef6f0` | 26052 | s1 (lockbox-1) | a withdrawal dressed as order fields: `unknown_kind` |
| `874fe64ff13f4e778be20ba10e75c69e` | 26270 | s1 (lockbox-1) | `over_max_order_size` |
| `ddb63525fc1f4b7983b5253cf3c0ce0e` | 26278 | s1 (lockbox-1) | `policy_revoked` (revoked at 26273) |
| `248bdc7870e94358b399a2ba537ceff6` | 26296 | s1 (lockbox-1) | `quorum_unavailable`: two signers down, nothing signs, not even a cancel |
| `d948fb350ff34fe98c1e705dacd47277` | 26872 | s3 (lockbox-3) | `nonce_not_above_highest_signed`: a signer restored from an old snapshot refuses to sign again |

### Transactions

Venue side, one per filled order. Each opens on Hyperliquid's explorer at `https://app.hyperliquid.xyz/explorer/tx/<hash>`:

| Venue order | Venue transaction hash |
|---|---|
| 598536092472 | tx 0xa255e0afc0773b31a3cf04485b9e0902028a00955b7a5a03461e8c027f7b151c |
| 598536117844 | tx 0xac63460b1d918974addd04485b9e4202014f00f0b894a846502bf15ddc95635f |
| 598603151430 | tx 0xd1601a9be087b9e0d2d904485d8c6802011800817b8ad8b27528c5ee9f8b93cb |
| 598603170550 | tx 0xa81eaf8cfc78c740a99804485d8c9d016600c772977be6124be75adfbb7ca12b |
| 601320050651 | tx 0x43b710a5ede129b5453004488911eb02018f008b88e44887e77fbbf8ace5039f |
| 601320087888 | tx 0xdf8607fdef9df0d2e0ff04488912250202c000e38a910fa4834eb350ae91cabd |

Chain side, one per decision. A signer that stood down recorded `quorum_met_without_this_signer` as a refusal. There is no public explorer for this chain; on a synced node, `curl -s "localhost:26657/tx?hash=0x<hash>"` returns the transaction:

| Request | Block | Signer (server) | Decision | Chain transaction hash |
|---|---|---|---|---|
| `9ef85b293f774ab5992f1e64709bde6e` | 26261 | s1 (lockbox-1) | signed | tx EA560C1DB0E1E7941FBC1A04661131402E4E80C3308C6980B98F16FD4D2C6D04 |
| `9ef85b293f774ab5992f1e64709bde6e` | 26261 | s3 (lockbox-3) | signed | tx 127FEBD441B90ECB6185F1D7FAD36E492ABDAB9EB9E96EB75FCE1A571D53138D |
| `9ef85b293f774ab5992f1e64709bde6e` | 26261 | s2 (lockbox-2) | stood down | tx 63086FA36D802A34231B994C49874D1E7B5C74E2BD4FA37E7B91F91A36699758 |
| `a31ed065d7324531b38ba5c352701a24` | 26265 | s1 (lockbox-1) | signed | tx 91CB1008473F93A7FCB2C9333200A566264739CB3FA59827869CAD34A652F78E |
| `a31ed065d7324531b38ba5c352701a24` | 26265 | s2 (lockbox-2) | signed | tx 7BCC0FF3814F01B3A67E1769D1DEA99923C8EB7280A3B3CE8E9F7F14C9E51E89 |
| `a31ed065d7324531b38ba5c352701a24` | 26264 | s3 (lockbox-3) | stood down | tx FA02538BF722494E71A5F784B8C6535BE31EB6587D342ADDF7634A73EF04FB4D |
| `91d46675868e425b9931b08c77a28122` | 33288 | s1 (lockbox-1) | signed | tx 4E3F53CAA24830B18357A51962A45A63F9ACA81B3ACFB5DEF81BBF019780AA07 |
| `91d46675868e425b9931b08c77a28122` | 33288 | s3 (lockbox-3) | signed | tx ABBC862014F1D2EBDE85681779EE08A6045766F60FB49FE5E934899EE9DADFE5 |
| `91d46675868e425b9931b08c77a28122` | 33287 | s2 (lockbox-2) | stood down | tx 3B0573B990DEA586633C72F8D3F7E8A595A1646ABB443A8BBDF81D5DFD2CD93D |
| `ddc2888e74e24430980d056fb4f89423` | 33291 | s1 (lockbox-1) | signed | tx EC503BE385A9A53303E0E09AF899990366619771CDAAF92251F1A442FCEAF886 |
| `ddc2888e74e24430980d056fb4f89423` | 33291 | s3 (lockbox-3) | signed | tx D5EF8861CF8DD817C9C25C0A350761A7E2F82199C8264E0CB125C2F8B8157E3A |
| `ddc2888e74e24430980d056fb4f89423` | 33290 | s2 (lockbox-2) | stood down | tx C2E9F4FB828E7B89B699BB1DB9A28E114AAA4B5DD90D294778F708B606E3747E |
| `2da216371a1646ef93e3489818bb7767` | 26051 | s1 (lockbox-1) | refused | tx 567DCDF5A0C2F826A0857EED4DFB4784FD5319AD369E6AA7C08381F1EFC29F38 |
| `52b0b57968e7443b8e6b131adbaef6f0` | 26052 | s1 (lockbox-1) | refused | tx 75F56D5B4ED68181B5C8067C2BC4AD0022AA8B10367C5A29348EB8FA2540E8D4 |
| `874fe64ff13f4e778be20ba10e75c69e` | 26270 | s1 (lockbox-1) | refused | tx 9B82A9F3651ACA86E250A2F49FFF1360E57A1C98B1B165FCA77B9C233F11E88E |
| `ddb63525fc1f4b7983b5253cf3c0ce0e` | 26278 | s1 (lockbox-1) | refused | tx E876169EB36508135F2214E801D5B084465D070FFA73FE3D047468461CB2E9CC |
| `248bdc7870e94358b399a2ba537ceff6` | 26296 | s1 (lockbox-1) | refused | tx 237AAE1D8514B3C9ADA39681DD1B0DA34AEF10F35C70392946145341A9A3FB12 |
| `d948fb350ff34fe98c1e705dacd47277` | 26872 | s3 (lockbox-3) | refused | tx 37BC7E036C118FCF75E0EFABADD8CA80D4016B6D4AEA0EF7035F312638B6883A |

### Who signed

There were 50 blocks carrying a decision, from height 365 to 33293:
- lockbox-1, lockbox-2 and lockbox-4 signed all 50.
- lockbox-3 signed 44. Its validator was stopped on purpose for the one-signer-down test.
- The external validator signed all 7 decision blocks after it joined.
- On 11 Nov, two more orders: decision blocks 188407 to 188413, with 12 decision transactions (`C1CB0AB9…`, `7D6AC1D0…`, `37E57229…`, `41E74256…`, `34856775…`, `9A6DA78B…`, `B626A9C4…`, `4ABDB14D…`, `1D271AEF…`, `F225E45D…`, `41BFAD1D…`, `5093655F…`). All 6 validators signed each of those blocks, the Grok bot's node among them.

Of the 34 signed rounds, s1 took part in 34, s2 in 21 and s3 in 13.

## Check it yourself

Sync a node from `docs/VALIDATOR_JOIN.md`. Then, on that node:

```
curl -s localhost:1317/lockbox/v1/keyref/kr-milestone-a-3
curl -s "localhost:26657/tx_search?query=\"lockbox.decision_recorded.request_id='ddc2888e74e24430980d056fb4f89423'\""
curl -s "localhost:26657/commit?height=33291" | jq '.result.signed_header.commit.signatures[].validator_address'
curl -s "localhost:26657/validators?height=33291"
```

On the venue's public API, no account needed:

```
curl -s -X POST https://api.hyperliquid.xyz/info -H 'Content-Type: application/json' \
  -d '{"type":"historicalOrders","user":"0x97a7c73667d002ca31c8f045c11d017b1bc2c904"}'
curl -s -X POST https://api.hyperliquid.xyz/info -H 'Content-Type: application/json' \
  -d '{"type":"userRole","user":"0xfce7a019fc4b20c2c646e393728e27f05cbfb06b"}'
```

## The main blocker found after Milestone A, and what we are doing about it

A Grok bot built a node from this repository and synced it on its own. It could not become a validator on its own. It had to relay its public validator key and node id through a chat room. Someone then had to notice the message, and the admin had to send the add by hand. The admin is a single key on one founding machine. Each hand-off is slow and easy to miss: the bot's first reply sat unread. It does not scale past one bot. No more Grok bots join until this is cleared.

A second finding came with it. The Grok node ran inside the bot's own session and stopped after under a minute of signing, most likely when the bot's turn ended. It could not be restarted once the bot's usage ran out. A validator has to keep running when nobody is watching it.

What we are doing, in this order:

1. **The join request goes on the chain.** A node sends `MsgRequestJoin`, signed with its own consensus key over a fixed domain string, the chain id, its account, its node id and its public key. The chain checks the signature and adds the node at the next block. There is no relay and no admin step. The limits:
   - 100,000 power each, at most 100 joiners;
   - outsiders together stay small enough that the founders keep the chain live;
   - one validator per account and one per IP;
   - a joiner that misses 300 blocks in a row is removed and can ask again after one day.
   This is built and tested on the `open-join` branch and is not live yet.
2. **Upgrades without restarting the chain.** The same release adds the upgrade module and cosmovisor. Later changes then arrive as an upgrade plan on the chain, and every node switches binary at the same height by itself. `lockbox-milestone-a-1` keeps running and is not restarted. This one release goes in by a coordinated binary swap at a set height, and that is the last manual swap.
3. **One file for a bot.** `SKILL.md` takes a bot from an empty machine to a synced node that sends its own join request.
4. **The node outlives the bot.** `SKILL.md` leaves the node running as a system service on a machine the bot rents, not inside the bot's session.
5. **The admin stops being one key.** The same release adds a two-step admin change: the current admin proposes a new address, and the change takes effect only when that address accepts it. It is built and tested on the `open-join` branch and is not live yet.

**Update, 11 Nov 2026.** This chain is not being upgraded. Lockbox continues as Signer, part of TraderSwift, on a new chain: https://github.com/traderswift-io. The join request and the two-step admin change are in it from the first block, so no coordinated binary swap is needed. The new set of signers generates its key jointly instead of importing it.

## What Milestone A does not prove yet

- The three signers are run by one operator, on one provider.
- The admin is one key.
- The key was imported, not generated jointly.
- The pair s2 plus s3 has not signed a round without s1.
- The chain binary is not yet checked against a release hash on the signer side.

## Closing record

- Chain `lockbox-milestone-a-1` ran from 8 Nov to 11 Nov 2026 and was retired on 11 Nov. Its full block history, genesis and validator-set history are archived.
- At retirement there were 6 validators: the 4 founding validators, the external validator and the Grok bot's node.
- 6 orders filled on the venue, each signed by 2 of 3 signers under the policy on chain. Each refusal is on chain with its reason.
- The chain halted at height 196500 at 07:29 UTC on 11 Nov 2026, when the four machines were destroyed. The signer machines, their key shares and every snapshot of them are gone.
