# Lockbox

[Milestone A results](docs/MILESTONE_A_RESULTS.md): what was proven and how to check it yourself.

Lockbox is part of TraderSwift.

## TraderSwift

TraderSwift is a trading network. Agents and people use it to trade across exchanges. It is built from subnets, and each subnet runs one job on its own set of nodes.

## What Lockbox is

Lockbox is the signing subnet. It holds a trading key without the whole key ever sitting in one place:

- The key is split into shares across a committee of signers. Any 2 of the 3 can sign together. One signer alone cannot.
- An owner writes a policy to a small blockchain. The policy says which account the key may trade, which order kinds are allowed, and the size caps.
- Each signer builds the exchange message itself from the order request and checks it against the policy on chain. It signs only if the policy allows the order, and transfers and withdrawals are always refused. Every refusal is recorded on the chain.
- Signing is interactive, with nothing signed in advance.

The pieces are the chain (Go, Cosmos SDK, with its own validator-set module), the signer (Rust, threshold ECDSA on `cggmp21`), a coordinator that collects signatures and sends the order (Python), and a one-time tool that imports an existing key as shares.

## Free and open source

Lockbox is free and open source under the Apache License 2.0. Use it however you like, including running it as your own blockchain with your own signers.

TraderSwift runs it as one of its subnets.

## Status

Lockbox is in dev mode. The chain is live as a development network, and we are testing orders on Hyperliquid. Expect changes and a fresh genesis for the production network.

## Run a validator

Start with [docs/VALIDATOR_JOIN.md](docs/VALIDATOR_JOIN.md). How the pieces talk to each other is in [docs/INTERFACES.md](docs/INTERFACES.md).
