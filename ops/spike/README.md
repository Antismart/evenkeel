# Phase 0 spike runbook — FNN Pudge testnet gate

Goal (CLAUDE.md Phase 0 / architecture §11): stand up FNN on Pudge testnet,
open 2–3 channels, execute one `dry_run` circular self-payment, then one real
one. Record everything in `docs/spike-notes.md`. **Green** → full plan.
**Red** (no circular routes findable) → advisory-only pivot.

This is human-led; these scripts just remove the boilerplate.

Prereqs: Docker (with compose), `curl`, `jq`, `openssl`. Optional but
recommended: `ckb-cli` (prints the faucet address during setup).

## 1. Start the node

```sh
cd ops/spike
./setup-fnn.sh              # generates key + .env, seeds config, docker compose up
docker compose logs -f fnn  # watch it sync/connect
```

Fund the printed testnet address at <https://faucet.nervos.org> (request a few
times; channels below need ~500 CKB each plus fees).

Sanity check the RPC:

```sh
./fnn-rpc.sh info           # should return version, pubkey, addresses
```

## 2. Open 2–3 channels

A circular route needs at least two channels from our node whose far ends can
reach each other. Easiest shape on a sparse testnet: open two channels to two
well-connected public nodes that share a channel with each other (check
https://testnet.explorer.nervos.org/fiber/graph or `./fnn-rpc.sh raw graph_channels '[{}]'`).

For each peer (multiaddress from the explorer / community node lists):

```sh
./fnn-rpc.sh raw connect_peer '[{"address": "<peer multiaddr>"}]'
./fnn-rpc.sh raw open_channel '[{
  "peer_id": "<peer id from the multiaddr>",
  "funding_amount": "0xba43b7400"
}]'                          # 0xba43b7400 = 50_000_000_000 Shannons = 500 CKB
./fnn-rpc.sh channels        # wait for state CHANNEL_READY
```

Funding takes a few blocks. `channels` shows decoded balances plus
`usable_out`/`usable_in` (the §5.1 formulas, netting pending TLCs).

Note: freshly opened channels have all liquidity on OUR side. That is fine for
the gate — a circular self-payment only needs outbound room on the exit
channel and *inbound* room on the return channel; the second channel's far end
must be able to send back to us, which requires the counterparty to have
balance toward us on SOME channel. If every return path lacks inbound
liquidity, either ask a counterparty to push funds / open a channel toward us,
or receive a small payment first. Record whatever you had to do — that's
exactly the friction the spike exists to surface.

## 3. The gate test

```sh
# Price it — moves nothing, proves a circular route exists:
./fnn-rpc.sh dry-run --amount 10000000000            # 100 CKB in Shannons

# Real send — dry-runs the SAME params first, shows the fee, asks, then sends
# with the node-side ceiling max_fee_amount = 1 CKB:
./fnn-rpc.sh rebalance --amount 10000000000 --max-fee 100000000

# Before/after evidence for the notes:
./fnn-rpc.sh channels
```

If `dry-run` fails with "no route", try smaller amounts, other channel pairs,
and (worth one attempt) the explicit-route path via
`raw build_router` + `raw send_payment_with_router` (architecture §4, path 1).
Persistent failure across all of that = RED.

## 4. Record the result

Fill in `docs/spike-notes.md` (template provided): exact RPC calls, responses,
payment hash, actual fee, before/after balances, and the GREEN/RED verdict.
If red, the pivot decision goes into CLAUDE.md before any Phase 1 code.

## Teardown

```sh
docker compose down          # keeps ./fiber-node (key + channel state!)
```

`fiber-node/` holds the node key and channel state — deleting it with open
channels strands testnet funds until force-close. It is gitignored; never
commit it.
