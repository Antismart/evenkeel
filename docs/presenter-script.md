# Presenter script — live judging demo (~5 min)

Companion to `docs/live-demo.md` (the operational runbook). This is what to
*say and do*, in order, against the real Pudge node. Bold lines are spoken;
bracketed lines are actions. Rehearsed end-to-end on 2026-07-07.

## Before judges arrive (2 min)

```sh
cd evenkeel
docker start evenkeel-spike-fnn                       # if not already up
EVENKEEL_POLL_INTERVAL_SECS=5 docker compose up -d
open http://localhost:3000
```

Sanity-check: header shows `0.8.1 · 0285605841…`, no stale banner, two channel
cards. Keep a terminal visible beside the browser — judges like seeing the
node answer directly.

## 1. The problem — 30s, dashboard on screen

**"This is Even Keel — channel liquidity management for Fiber node operators.
The FNN README lists this as an unchecked TODO. On Lightning this problem grew
a whole tool category, because a depleted channel silently stops forwarding
and bleeds routing revenue. What you're looking at is my real Fiber node on
Pudge testnet — these are live channel balances, polled every five seconds."**

[Point at a channel card: the usable-ratio meter, the peer pubkey, the sparkline.]

**"The key number is *usable* liquidity — net of in-flight HTLCs, not raw
balance. And the sparkline tracks drift, so a channel at 60% and falling fast
gets flagged before it's actually dead."**

## 2. Prove it's live — 20s, terminal

```sh
cd ops/spike && ./fnn-rpc.sh channels
```

**"Same numbers, straight from the node's RPC — no middleman."**
[Numbers match the cards.]

## 3. Wake it up with policy — 45s

**"Right now both channels sit inside my policy's healthy band, so the tool
correctly does nothing — it only ever spends when there's measurable
imbalance. Let me tighten the policy — this is a real API the operator owns."**

```sh
cd .. && curl -s localhost:3030/api/policy | jq \
  '.target_ratio_bp = 8150 | .depleted_below_bp = 8050 | .saturated_above_bp = 8300' \
  | curl -s -X PUT -H 'Content-Type: application/json' -d @- localhost:3030/api/policy
```

(Adjust the three numbers to bracket wherever the channels currently sit:
depleted threshold just above the lower channel's ratio, saturated just below
the higher one's, target in between.)

[Within ~10 s a card reclassifies and the proposal appears.]

**"There it is. The planner paired my oversupplied channel with the starved
one, sized the amount so neither overshoots the target — and that fee is not
an estimate. It asked the node to price the exact payment with a dry run.
Nothing has moved yet."**

## 4. The money moment — 60s

[Hover over the proposal card; read out amount and fee.]

**"Advisory is the default: nothing moves without this click. And the fee I
approve is a ceiling the node itself enforces — even if my code were wrong, it
can't overspend it. A failed rebalance costs nothing at all; that asymmetry is
the whole safety model."**

[Click **Approve & send**. Action log: `submitting → confirming → settled`,
~10–15 s.]

**"Settled. Real circular self-payment on Pudge — there's the payment hash,
and the actual fee matches the quote to the shannon. It goes into a daily
budget ledger; the worst case of any bug in this tool, ever, is that budget."**

[Terminal: `cd ops/spike && ./fnn-rpc.sh channels` — balances moved on the node.]

## 5. Autopilot — 30s

[Flip the toggle in the policy panel.]

**"Autopilot is opt-in and off by default. Same state machine, same budgets —
it just doesn't wait for my click. And every autopilot action logs the exact
policy snapshot that authorized it, so the audit trail answers 'why did it
move my money' forever."**

[If a second proposal triggers, let it settle unattended; point at
`mode: autopilot` in the log. If not:]
**"Nothing qualifies right now — which is the point: no imbalance, no spend."**

## 6. Close — 30s

**"Everything you watched is the production code path — the same executor
passes a seven-scenario failure suite and a property test that says no policy
can ever spend fees without reducing imbalance. One command brings this up
from a clean clone. Even Keel: advisory by default, autopilot when you trust
it, bounded worst case always."**

## If testnet misbehaves mid-demo

The dry run fails → the proposal is **rejected with the reason in the log and
zero spend**. Don't apologize — point at it:

**"That's the safety model working: it couldn't price the route, so it refused
to send. Here's the same flow settling in yesterday's rehearsal."**

[Show the rehearsal settlement — payment hash `0x47ac455f…c3d5c7` in
`docs/live-demo.md` — then fall back to `ops/sim/report.html` for the
24-hour story.]

## Reset between run-throughs

Each rebalance moves a few CKB from the higher channel to the lower one. To
re-demo, tighten the band again around wherever the channels now sit (the
three `jq` numbers are the knob). Inbound headroom on the return channel is
~80 CKB — good for several rounds; if it runs low, one keysend push per
`ops/spike/README.md` replenishes it.
