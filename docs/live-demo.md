# Live demo runbook — judging window

Organizer ruling: a live working demo is **mandatory**; the backend node must
be live during the judging window (not continuously). This procedure was
rehearsed end-to-end on 2026-07-07: Even Keel's executor planned, priced,
and settled a real 19.9 CKB rebalance on Pudge testnet — payment hash
`0x47ac455f4ac34d6f81b5f446cdd8e02f90d400a70af08c754528094448c3d5c7`, actual
fee 1,990,000 Shannons (exactly the dry-run quote), sink channel landing at
exactly the 80.00% policy target.

## Prerequisites (already in place)

- Funded testnet node key + config in `ops/spike/fiber-node/` (~99,000 CKB
  on-chain, two open channels to bootnodesgp totaling ~652 CKB local).
- Dev/demo Postgres (`evenkeel-dev-pg`) or the compose `postgres` service.

## Bring-up (T-15 minutes before judging)

```sh
# 1. The backend FNN node (skip if already running):
cd ops/spike && docker start evenkeel-spike-fnn || ./setup-fnn.sh
./fnn-rpc.sh info          # sanity: version 0.8.1, pubkey 0285…3cef
./fnn-rpc.sh channels      # both channels ChannelReady

# 2. Even Keel against it, real mode:
cd ../.. && export DATABASE_URL=postgres://evenkeel:evenkeel@127.0.0.1:5433/evenkeel
docker start evenkeel-dev-pg
EVENKEEL_NODE_MODE=real EVENKEEL_FNN_URL=http://127.0.0.1:8227 \
  EVENKEEL_POLL_INTERVAL_SECS=5 cargo run -p evenkeel-server --release &
(cd dashboard && pnpm dev &)          # or the built .output server
open http://localhost:3000            # real channels, real balances
```

## Staging a visible rebalance (the policy is the knob)

Fresh channels sit wherever the last rebalance left them (both near 80%
after the rehearsal). Rather than shuffling funds, tighten the policy band so
one channel classifies as a sink — this is a *feature demo*, not staging
trickery: judges watch a policy change take effect live.

```sh
# Pull current policy, tighten the band around the channels' actual ratios:
# target just above the lower channel's ratio; depleted threshold between them.
curl -s localhost:3030/api/policy | jq \
  '.target_ratio_bp = 8300 | .depleted_below_bp = 8100 | .saturated_above_bp = 8500' \
  | curl -s -X PUT -H 'Content-Type: application/json' -d @- localhost:3030/api/policy
```

Within two ticks the lower channel classifies `depleted`, the planner pairs
it with the higher one, prices via the node's real `dry_run`, and the
proposal card appears with the live fee quote. **Check the numbers before
clicking**: amount ≈ tens of CKB, fee ≈ 0.1% of it. Click **Approve & send**;
settlement lands in seconds; the action log shows the payment hash and the
actual fee; `./fnn-rpc.sh channels` confirms the on-node shift.

If asked, flip the autopilot toggle and repeat with a re-tightened band — the
next proposal settles with no click, logged `mode: autopilot` with its policy
snapshot.

## Known constraints, stated honestly

- Both demo channels peer with the same node (bootnodesgp) — the circle is
  us → bootnodesgp → us. Public-testnet peers that hold sessions are scarce
  (see spike notes); the two-channel circle exercises the identical
  `send_payment(allow_self_payment)` primitive as any longer route.
- Inbound liquidity on the return channel is finite (~80–100 CKB). Each
  rebalance consumes sink-side inbound; a keysend push
  (`./fnn-rpc.sh raw send_payment …` per spike README) replenishes it if
  multiple live rebalances are wanted in one session.
- If testnet misbehaves during the window (peer down, no route), the dry run
  fails and Even Keel *rejects the intent and spends nothing* — that safety
  behavior is itself demonstrable; the simulation report and the recorded
  rehearsal evidence carry the rest.

## Teardown after judging

```sh
kill %1 %2 2>/dev/null                 # evenkeel-server + dashboard dev
# Leave the FNN node running or stop it: docker stop evenkeel-spike-fnn
# (fiber-node/ holds the key and channel state — never delete with channels open)
```
