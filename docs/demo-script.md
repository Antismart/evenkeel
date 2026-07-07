# Demo video script

Target: ~4 minutes. Every claim on screen is backed by something in the repo;
the "what's real vs simulated" line is spoken out loud (the brief rewards it).

## Setup before recording

```sh
git clone https://github.com/Antismart/evenkeel && cd evenkeel
# Scripted mock for a tight, reproducible recording (live is the repo default;
# for a live-node recording follow docs/live-demo.md instead):
EVENKEEL_NODE_MODE=mock EVENKEEL_POLL_INTERVAL_SECS=2 docker compose up -d   # 2s ticks → proposal in ~1 min
open http://localhost:3000
```

Have two browser tabs ready: the dashboard, and `ops/sim/report.html`.

## Shot list

**1. The problem (30s) — slide or README intro on screen.**
> "Fiber channels drift as payments route through them. A depleted channel
> silently stops forwarding — you lose routing revenue and nobody tells you.
> The FNN README literally lists channel liquidity management as an unchecked
> TODO. Lightning grew a whole tool category for this. Fiber had nothing —
> until Even Keel."

**2. Monitoring (45s) — dashboard tab.**
- Point at the three channel cards: healthy, draining, saturated.
- Point at the drift sparkline on the draining card: "It's at 60% and falling
  — Even Keel classifies it *depleting* from the drift slope, before it's
  actually depleted. That's the difference between a monitor and a
  read-current-balance script."
- Point at the staleness banner logic and `/metrics` (open
  `localhost:3030/metrics` briefly): "The tool that watches your channels is
  itself watchable."

**3. The money path (60s) — the proposal card appears.**
- "The planner paired the saturated channel with the draining one, sized the
  amount so neither overshoots the 50% target, and — this matters — priced it
  with the node's own dry-run before proposing. That fee is a quote, and the
  node re-enforces it as a hard ceiling on send."
- Click **Approve & send**. Watch the action log: submitting → confirming →
  settled, actual fee equal to the quote, budget ledger ticking up.
- "Failure costs nothing — a failed self-payment simply doesn't settle. The
  worst case of any bug in this tool is the daily fee budget. That asymmetry
  is why it's safe to automate at all."

**4. Autopilot (30s) — policy panel.**
- Flip the autopilot switch. "Opt-in, default off, persisted. Same state
  machine, same budgets — it just skips waiting for my click. And every
  autopilot action logs the exact policy snapshot that authorized it."
- Wait for the next proposal to settle unattended; show `mode: autopilot` in
  the log.

**5. Honesty + evidence (45s) — split between spike notes and sim report.**
- "What you just watched runs against a scripted mock node — deliberately, so
  the demo is reproducible. But the real thing works: here's the Phase 0
  spike settling a real 100-CKB circular rebalance on Pudge testnet, actual
  fee exactly matching the quote." (Show `docs/spike-notes.md`, the payment
  hash and balance table.)
- Switch to `ops/sim/report.html`: "And here's what a full day buys you —
  deterministic 24-hour replay through the real executor: steady drain ends
  at 34% mean imbalance managed versus 42% unmanaged, for 0.44 CKB in fees.
  On oscillating traffic it correctly spends almost nothing — the hysteresis
  refuses to chase traffic that would undo each rebalance. A property test
  pins that: no policy can spend fees without net imbalance reduction."

**6. Close (20s).**
> "One command brings all of this up from a clean clone. Advisory by default,
> autopilot when you trust it, bounded worst case always. Even Keel — keep
> your Fiber node on an even keel."

## Timing note

With `EVENKEEL_POLL_INTERVAL_SECS=2` the draining channel crosses target
~50 s after startup; the first proposal follows on the next tick. Record shot
2 while waiting.
