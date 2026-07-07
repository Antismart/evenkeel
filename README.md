# Even Keel

**Channel liquidity manager for [Fiber Network](https://github.com/nervosnetwork/fiber) node operators.**

Fiber channels drift out of balance as payments route through them. A depleted channel silently stops forwarding in one direction — it earns nothing, causes upstream payment failures, and nobody tells you it happened. The FNN README lists "advanced channel liquidity management" as an unchecked TODO; Lightning grew a whole tool category for this (lndmanage, charge-lnd, bos rebalance). Fiber has nothing yet. Even Keel is that tool.

It watches your channels' balance drift over time, classifies their health (healthy / depleting / depleted / saturated), and corrects imbalance with **circular self-payments** — a rebalancing primitive the FNN RPC supports natively (`send_payment` with `allow_self_payment`). Advisory by default: it proposes a rebalance with an exact fee quote and you click to approve. Autopilot is opt-in and budget-bounded.

## Why it's safe to let a tool touch your liquidity

The economics of a circular self-payment are asymmetric, and everything is built on that:

- **Success costs exactly the routing fee** — quoted upfront via `dry_run: true` before any real send, and enforced twice (our budget ledger *and* the node-side `max_fee_amount`).
- **Failure costs nothing** — an unroutable or failed self-payment simply doesn't settle; principal never moves.
- **Worst case under any combination of bugs: the configured daily fee budget.** Even Keel holds no keys; it talks to your own node's RPC.

Verified on Pudge testnet (2026-07-02): a real 100 CKB circular rebalance settled with the actual fee **exactly matching the dry-run quote** (0.1 CKB), principal untouched. Full evidence in [`docs/spike-notes.md`](docs/spike-notes.md).

## Status

Built for the "Gone in 60ms" Fiber Infrastructure Hackathon (Category 3: Liquidity), designed to outlive it.

| Phase | Scope | State |
|---|---|---|
| 0 — Testnet gate | Prove a real circular self-payment settles on Pudge testnet | ✅ **GREEN** ([evidence](docs/spike-notes.md)) |
| 1 — Monitoring spine | Poller, usable-liquidity health engine, drift detection, `/metrics`, dashboard | ✅ done |
| 2 — Money path | Planner, serialized executor state machine, advisory flow, fee ledger | ✅ done |
| 3 — Autopilot + simulation | Opt-in autopilot with budgets; deterministic 24h simulation harness | ✅ done |
| 4 — Ship | Hosted demo, video, submission | ▶ next |

The authoritative design is [`docs/architecture.md`](docs/architecture.md) — system shape, decision core, executor state machine, failure handling, and the ADRs behind every non-obvious choice.

## Quickstart

One command, no Fiber node, no tokens (the MockNode demo scenario, ADR-6):

```sh
docker compose up
# dashboard: http://localhost:3000   API + /metrics: http://localhost:3030
```

You get three scripted channels — healthy, steadily draining (watch it classify
`depleting` from drift before it's actually depleted), and saturated — with live
sparklines and Prometheus gauges. Once the draining channel dips below target,
the planner proposes a rebalance: a card appears with the pair, amount, and the
**exact dry-run fee**; click Approve and watch it move through
`submitting → confirming → settled` in the action log, with the actual fee
entering the daily budget ledger. Advisory is the default — nothing is ever
sent without the click. The policy panel exposes the **autopilot** switch
(opt-in, persisted, off by default): flipped on, priced rebalances within
budget execute unattended, each logged with `mode: autopilot` and the exact
policy snapshot that authorized it. Against a real node: set
`EVENKEEL_NODE_MODE=real` and run with `--profile testnet` (builds FNN v0.8.1
from source; see comments in `docker-compose.yml`).

### The simulation (what a day of Even Keel buys you)

```sh
cargo run -p evenkeel-server --bin sim --release
```

A deterministic 24-hour day (288 five-minute ticks) replayed against three
scripted traffic patterns — steady drain, burst, oscillating — twice each:
unmanaged, and managed by the **real** executor/planner/store code paths (no
simulation-only logic touches a decision). Output: `ops/sim/report.html`, a
self-contained with/without balance-ratio trajectory chart, committed in-repo.
Sample result: steady drain ends the day at 34.3% mean imbalance managed vs
41.7% unmanaged, for 0.44 CKB in fees — and on the oscillating pattern the
cooldown correctly refuses to chase traffic that would undo each rebalance.
The §8 property test pins the safety claim: for any policy, fees never exceed
the daily cap and net imbalance never increases over a simulated day.

### Development

```sh
docker run -d --name evenkeel-dev-pg -p 127.0.0.1:5433:5432 \
  -e POSTGRES_PASSWORD=evenkeel -e POSTGRES_USER=evenkeel -e POSTGRES_DB=evenkeel postgres:16-alpine
export DATABASE_URL=postgres://evenkeel:evenkeel@127.0.0.1:5433/evenkeel
cargo test --workspace     # property tests included; store tests need DATABASE_URL
cargo run -p evenkeel-server                    # mock mode on :3030
cd dashboard && pnpm install && pnpm dev        # dashboard on :3000
```

Builds work without a database (`SQLX_OFFLINE=true`, `.sqlx` is committed).

## Repository layout

```
crates/
  evenkeel-core/    pure decision core: usable liquidity, health classes, drift
                    (no I/O; property-tested)
  evenkeel-node/    FiberRpc trait; RealNode (FNN JSON-RPC) + MockNode (scripted,
                    fault-injectable — the dev/CI/demo environment, ADR-6)
  evenkeel-store/   Postgres snapshot time-series (u128-exact NUMERIC columns)
  evenkeel-server/  serialized control loop, REST API, Prometheus /metrics
dashboard/          Nuxt 3 operator dashboard: channel cards, drift sparklines,
                    staleness banner
migrations/         sqlx migrations
ops/spike/          Phase 0 spike: standalone FNN testnet node + fnn-rpc.sh,
                    a manual JSON-RPC driver for circular rebalances
docs/
  architecture.md   The design. Read this first.
  spike-notes.md    Phase 0 gate evidence: what ran, what it cost, what broke.
```

## Try the testnet spike yourself

Prereqs: Docker, `curl`, `jq`, `openssl` (optional: `ckb-cli` to print the faucet address).

```sh
cd ops/spike
./setup-fnn.sh          # keys, config, builds FNN v0.8.1, starts the node
./fnn-rpc.sh info       # node identity
./fnn-rpc.sh channels   # balances with usable-liquidity breakdown
# fund at https://faucet.nervos.org, open channels (see ops/spike/README.md), then:
./fnn-rpc.sh dry-run   --amount 10000000000                       # price a circle, moves nothing
./fnn-rpc.sh rebalance --amount 10000000000 --max-fee 100000000   # dry-run, confirm, send, settle
```

The RPC port binds host-loopback only — it can spend your node's funds; never expose it.

## What's real vs what's simulated

The hackathon brief rewards saying this plainly, so:

**Real, proven on Pudge testnet (2026-07-02, [evidence](docs/spike-notes.md)):**
- A circular self-payment rebalance settled on FNN v0.8.1 via
  `send_payment(allow_self_payment, keysend)` — payment hash `0x5888…cc54`,
  actual fee exactly equal to the dry-run quote (0.1 CKB on 100 CKB moved).
- The RPC client (`RealNode`), the funding/channel-open flow, and the
  loopback-only security posture — all exercised against a live node.

**Real code, scripted environment (the default demo):**
- Every decision, execution, persistence, and safety code path in the demo is
  the production code — the same planner, the same §6 executor state machine,
  the same budget ledger. Mock mode swaps exactly one component behind the
  `FiberRpc` trait: the node (scripted balances, deterministic 0.1% fees,
  instant settlement). Nothing decision-relevant is stubbed.
- The 24h simulation (`ops/sim/report.html`) replays scripted traffic through
  those same real code paths, deterministically.

**Why the demo defaults to mock:** public Fiber testnet routing is sparse and
peer quality uneven (documented first-hand in the spike notes — unreachable
announced addresses, peers failing the Init handshake). The tool runs against
a real node today (`--profile testnet`), but a reproducible demo should not be
hostage to testnet weather (ADR-6). The Phase 0 settlement is the proof the
real path works; the mock is the proof the judgment layer works.

## The gap this addresses

The [FNN README](https://github.com/nervosnetwork/fiber) lists **"advanced
channel liquidity management"** as an unchecked TODO. On Lightning, that gap
grew a whole tool category (lndmanage, charge-lnd, bos rebalance) because
routing nodes bleed revenue without it. Fiber has the primitive (self-payment
rebalancing is documented in the RPC) but no tooling. Even Keel is the first:
monitoring, drift detection, priced proposals, budget-bounded autopilot, and
an audit trail — advisory by default, everything explainable.

## Design principles (the short version)

- **One serialized control loop; at most one rebalance in flight, ever.** Explainability over throughput — every action traces to one snapshot, one plan, one price. (The testnet spike independently endorsed this: concurrent channel funding races a single wallet cell and loses.)
- **Money is `u128` Shannons everywhere; ratios are integer basis points.** Floats are display-only.
- **Decisions use usable liquidity** (net of in-flight TLCs), not raw balance.
- **The mock node is a first-class artifact** — the decision core is fully testable with no network, no testnet, no tokens.

## Roadmap beyond v1

Passive rebalancing via fee policy (`update_channel`), min-cost-flow planning at higher channel counts, fleet mode, LSP liquidity primitives. Details in [architecture §12](docs/architecture.md).
