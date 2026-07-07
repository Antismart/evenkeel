# Submission checklist — "Gone in 60ms" (Category 3: Liquidity)

> **DRAFT against the brief.** The canonical nine graded deliverables live in
> the hackathon brief; this maps what the repo already contains onto the
> usual grading axes. Reconcile the left column against the brief's exact
> wording before submitting and fill the two ⬜ human items.

| # | Deliverable | Where | Status |
|---|---|---|---|
| 1 | Working code, public repo | github.com/Antismart/evenkeel — 4 Rust crates + Nuxt dashboard, 75 tests, clippy-clean | ✅ |
| 2 | One-command run | `docker compose up` — verified from a clean clone (2026-07-07) | ✅ |
| 3 | README: problem, what's real vs simulated, gap addressed, roadmap | README sections: intro, "What's real vs what's simulated", "The gap this addresses", "Roadmap beyond v1" | ✅ |
| 4 | Design documentation | `docs/architecture.md` — full design + 7 ADRs; `docs/spike-notes.md` — testnet evidence | ✅ |
| 5 | On-chain / on-network proof | Real circular self-payment settled on Pudge, payment hash `0x5888472682b1b6dbc954c625ae44dd2ff4d1a57a64985d6b3832352d3684cc54`, exact-fee settlement (spike notes) | ✅ |
| 6 | Demo video | Script ready: `docs/demo-script.md` (~4 min shot list) | ⬜ record |
| 7 | **Live demo (mandatory, organizer ruling 2026-07-07)** | `docs/live-demo.md` — judging-window runbook, rehearsed end-to-end: Even Keel settled a real 19.9 CKB rebalance on Pudge (hash `0x47ac455f…`, fee exactly the quote, sink landed on the 80.00% target) | ✅ rehearsed; run during window |
| 8 | Safety/production judgment | Advisory default (ADR-4), bounded worst case (§4), budgets enforced twice, §7 crash recovery, audit trail with policy snapshots, §8 property tests | ✅ |
| 9 | Post-hackathon viability | Architecture §12 roadmap (passive fee-policy rebalancing, min-cost-flow, fleet mode, LSP primitives); built on stable v0.8.x APIs | ✅ |

## Hosted demo — deployment notes (secondary to the live demo)

The mock-mode stack is self-contained and safe to host publicly (no keys, no
funds, no node): any Docker host works.

```sh
git clone https://github.com/Antismart/evenkeel && cd evenkeel
EVENKEEL_POLL_INTERVAL_SECS=10 docker compose up -d
```

Reverse-proxy `:3000` (dashboard) behind TLS; `:3030` (API/metrics) binds
host-loopback only in the compose file — expose it through the same proxy
only if the demo should show `/metrics`, and never expose a real-node
deployment's API without auth (architecture §9: basic auth or LAN-bind is the
operator's call; the internet-facing surface is the proxy's job).

For a testnet-backed demo instead: `ops/spike/` stands up the funded node
(key + faucet + channels per its README), then `--profile testnet` with
`EVENKEEL_NODE_MODE=real`. Budget the demo-day risk: routing depends on
public testnet peer health — the mock demo is the deterministic fallback
(ADR-6), and the spike notes are the proof the real path settles.

## Pre-submission verification (run all; all must pass)

```sh
git clone <repo> fresh && cd fresh
docker compose up -d                      # stack healthy, proposal appears
export DATABASE_URL=postgres://evenkeel:evenkeel@127.0.0.1:5433/evenkeel  # dev pg
cargo test --workspace                    # 75 passing
cargo clippy --workspace --all-targets   # clean
cargo run -p evenkeel-server --bin sim --release && cargo run -p evenkeel-server --bin sim --release
                                          # byte-identical ops/sim/report.*
```
