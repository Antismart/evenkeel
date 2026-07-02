# Even Keel — Architecture

**Status:** Design for build. Targets the "Gone in 60ms" Fiber Infrastructure Hackathon (Category 3: Liquidity), designed to outlive it.
**Author:** Antismart
**Target:** Fiber Network Node (FNN) v0.8.x, JSON-RPC
**Last updated:** June 2026

---

## 0. Summary

Even Keel is a channel liquidity manager for Fiber node operators. Channels drift out of balance as payments route through them; a depleted channel silently stops forwarding in one direction, losing routing revenue and causing upstream payment failures. Even Keel monitors channel balances over time, detects imbalance and drift, and corrects them with circular self-payments — a rebalancing primitive the FNN RPC supports as a documented, first-class feature (`send_payment` with `allow_self_payment`, and `send_payment_with_router` whose docs include a rebalancing recipe).

The system is deliberately shaped as: **a pure decision core** (health classification, rebalance planning — no I/O, fully testable), behind **two adapters** (the FNN RPC, the datastore), driven by **one serialized control loop**, with **a dashboard and metrics** on the side. Everything hard about moving money atomically is delegated to the node; everything Even Keel adds is judgment, safety bounds, and visibility.

---

## 1. Problem, goals, non-goals

### 1.1 The problem

A Fiber channel's capacity is split between `local_balance` (what you can send) and `remote_balance` (what you can receive). Routing traffic shifts the split. Three failure modes:

1. **Depletion** — `local_balance` → 0: the channel can't forward outbound. Half-dead, earns nothing outbound, causes pathfinding failures for others.
2. **Silent revenue loss** — you're never told you *would have* routed a payment if liquidity had been there. The cost is invisible without tooling.
3. **Capital inefficiency** — liquidity stacked on one side of several channels is locked capital earning nothing.

The FNN README lists "advanced channel liquidity management" as an unchecked TODO. Lightning grew a whole tool category here (lndmanage, charge-lnd, bos rebalance). Fiber has nothing yet.

### 1.2 Goals

- **G1 Visibility:** per-channel balance ratio, *usable* liquidity (net of in-flight TLCs), and historical drift.
- **G2 Detection:** classify channels (healthy / depleting / depleted / saturated) and detect drift *velocity* so action happens before a channel dies.
- **G3 Correction:** plan and execute circular rebalances with explicit fee economics; advisory by default, autopilot opt-in.
- **G4 Safety:** bounded worst case. A total logic failure can cost at most the configured fee budget; principal is never at risk (a failed self-payment simply doesn't settle).
- **G5 Testability without a network:** the decision core must be verifiable with no FNN, no testnet, no tokens.

### 1.3 Non-goals (v1)

- Fee-policy management (`update_channel` to attract rebalancing traffic passively) — v2, noted in §12.
- Channel-open recommendations, watchtowers, network exploration — adjacent tools.
- Custody of anything. The tool talks to the operator's own node RPC; it holds no keys.
- Multi-node fleet management — the schema anticipates it; v1 is one node.

---

## 2. Platform facts the design is grounded in

Confirmed against FNN RPC docs (v0.8.x). These are load-bearing; if any changes upstream, the affected component is named in parentheses.

| Fact | Consequence |
|---|---|
| `list_channels` returns `local_balance`, `remote_balance`, `offered_tlc_balance`, `received_tlc_balance`, `state`, `enabled`, counterparty `pubkey` | Poller + health engine have everything needed; **usable balance must net out TLC fields** (§5.1) |
| `send_payment(allow_self_payment: true, target: self, keysend: true)` is documented *"useful for channel rebalancing"* | The atomic rebalance primitive is the node's job, not ours (Executor) |
| `send_payment(dry_run: true)` returns fee + routability without sending | Every action is priced before commitment (Planner, Executor) |
| `max_fee_amount` / `max_fee_rate` enforced by the node | Fee ceiling has defense-in-depth: our budget check AND the node's (Executor) |
| `build_router` + `send_payment_with_router` allow explicit circular hops | Deterministic control over which channels a rebalance enters/exits (Planner) |
| `get_payment(payment_hash)` / `list_payments` report `Created / Inflight / Success / Failed` + actual fee | Settlement tracking and crash reconciliation (§7) |
| `graph_channels` exposes `ChannelUpdateInfo.outbound_liquidity`, `fee_rate` per direction | Route candidate selection without probing (Planner) |
| Amounts are `u128` Shannons; channels may be UDT-funded (`funding_udt_type_script`) | Money is `u128` end-to-end; **rebalances never cross assets** (§5.4) |
| FNN docs: RPC port must be restricted to trusted machines | Deployment is co-located, RPC never exposed (§9) |

---

## 3. System shape

```
                       ┌──────────────── evenkeel-core (pure, no I/O) ───────────────┐
                       │                                                              │
                       │   HealthEngine          RebalancePlanner        Policy       │
                       │   ratio, usable         pair selection,         budgets,     │
                       │   liquidity, drift,     amount sizing,          thresholds,  │
                       │   classification        benefit/fee ranking     modes        │
                       │                                                              │
                       └───────────▲──────────────────────┬──────────────────────────┘
                                   │ snapshots             │ intents
                                   │                       ▼
   ┌────────────┐  RPC   ┌─────────┴────────┐     ┌────────────────┐      ┌──────────────┐
   │  FNN node  │◀──────▶│  evenkeel-node    │     │   Executor      │─────▶│ evenkeel-store│
   │ (operator's│        │  FiberRpc trait:  │◀───▶│  serialized,    │      │ Postgres/     │
   │   own)     │        │  Real | Mock      │     │  state machine  │      │ SQLite:       │
   └────────────┘        └──────────────────┘     └────────┬───────┘      │ snapshots,    │
                                                            │              │ actions, policy│
                                   ┌────────────────────────┘              └──────▲───────┘
                                   ▼                                              │
                          ┌─────────────────┐        ┌────────────────┐          │
                          │  Control Loop    │        │  API + /metrics │──────────┘
                          │  tick: poll →    │        │  (Axum)         │◀──── Dashboard (Nuxt)
                          │  classify → plan │        └────────────────┘
                          │  → (approve) →   │
                          │  execute → log   │
                          └─────────────────┘
```

Ports-and-adapters, three crates plus frontend:

- **`evenkeel-core`** — pure domain logic. No tokio, no reqwest, no sqlx. Takes snapshots in, emits intents out. This is where correctness lives and where all property tests run (§8).
- **`evenkeel-node`** — the `FiberRpc` trait plus two implementations: `RealNode` (reqwest against FNN JSON-RPC) and `MockNode` (scripted balances, deterministic fees, fault injection). The mock is a first-class deliverable, not test scaffolding — it is how the tool is developed and demoed independent of testnet conditions.
- **`evenkeel-store`** — snapshot time-series, action log, policy, daily fee ledger. Postgres primary; SQLite feature-flag for single-binary operator installs.
- **`evenkeel-server`** — Axum: REST for the dashboard, Prometheus `/metrics`, and the control loop host.
- **`dashboard/`** — Nuxt 3, reusing the cellora-nuxt component library.

**The control loop is single-threaded by design** (one tick: poll → classify → plan → execute-at-most-one → log). See ADR-2.

---

## 4. The rebalance mechanic (what the node does for us)

A circular self-payment out through over-funded channel S and back through depleted channel D:

```
me ──(out via S)──▶ hop(s) ──(in via D)──▶ me
```

Settlement decreases S's `local_balance` and increases D's by the amount; total node balance changes only by the routing fee. Failure modes are benign: an unroutable or failed payment moves nothing. This asymmetry (success costs a known fee, failure costs nothing) is what makes an automated tool safe to build at all.

Execution paths, in order of preference:

1. **Explicit route:** `build_router` with hops pinned to exit S and enter D → `send_payment_with_router(keysend: true)`. Deterministic; the planner's chosen channels are honored exactly.
2. **Node-routed fallback:** `send_payment(allow_self_payment: true, target: self, keysend: true)` — simpler, but the node picks the route, which may not exit/enter the channels we intended. Used only if explicit routing fails and only when *any* rebalance of the pair is better than none. Logged distinctly.

Both paths are always preceded by the same call with `dry_run: true` to obtain the fee and routability. No exceptions — pricing before commitment is an invariant.

---

## 5. The decision core

### 5.1 Usable liquidity, not raw balance

Raw `local_balance` overstates what can move: funds locked in pending outgoing TLCs are not spendable. All decisioning uses:

```
usable_out(c) = local_balance(c)  - offered_tlc_balance(c)
usable_in(c)  = remote_balance(c) - received_tlc_balance(c)
capacity(c)   = local_balance(c) + remote_balance(c)          // per asset
ratio(c)      = local_balance(c) / capacity(c)                 // display
usable_ratio(c) = usable_out(c) / capacity(c)                  // decisions
```

Getting this wrong produces rebalances that fail against locked liquidity or, worse, oscillation. Money math is `u128` Shannons throughout; ratios are computed in scaled integer basis points (0–10_000), never floats, in any code path that feeds a decision. Floats are display-only. (Classic money-handling discipline; cheap to do from day one, expensive to retrofit.)

### 5.2 Health classification

Per channel, against policy thresholds (defaults shown):

```
usable_ratio < 0.20               → DEPLETED    (needs outbound refill)
usable_ratio > 0.80               → SATURATED   (rebalance source)
drift toward an edge, |slope| high → DEPLETING / FILLING (act early)
else                               → HEALTHY
```

Drift = slope of `usable_ratio` over the recent snapshot window (simple linear regression over N points). Drift is the feature that distinguishes this from a "read current balance" script: a channel at 0.35 and falling fast is a better rebalance target than one parked at 0.22. Classification is a pure function `(snapshot_window, policy) → Vec<ChannelHealth>`; property tests in §8.

### 5.3 Planning as an explicit (simplified) optimization

The general problem — given n channels with deviations from target ratio, find the set of circular transfers minimizing total deviation subject to fee budget — is a min-cost flow problem. **v1 deliberately does not solve it.** For an operator node with < ~20 channels, greedy pairwise selection captures nearly all the value at a fraction of the complexity (ADR-3):

```
1. candidates_D = channels DEPLETED or DEPLETING, sorted by (deficit × capacity) desc
2. candidates_S = channels SATURATED or FILLING,  sorted by (surplus × capacity) desc
3. for the top pair (D, S), same asset only:
     amount = min(
        surplus_above_target(S),        // don't push S below target
        deficit_below_target(D),        // don't overshoot D past target
        route_capacity_estimate,        // from graph outbound_liquidity
        policy.max_amount_per_action,
     )
4. price via dry_run → fee
5. benefit = imbalance_reduced(D, S, amount)   // in capacity-weighted bp
   accept iff benefit / fee ≥ policy.min_benefit_ratio
          and fee ≤ policy.max_fee_per_action
          and fee + spent_today ≤ policy.max_fee_per_day
6. emit at most ONE intent per tick
```

One intent per tick is a correctness choice, not a throughput limit: every executed rebalance invalidates the snapshot the plan was made from, and channel liquidity along candidate routes is shared state. Serial execution makes the system's behavior explainable — every action traceable to one snapshot, one plan, one price. (ADR-2.)

**Hysteresis:** a rebalanced pair is cooled down for M ticks and re-trigger thresholds are offset from target (act below 0.20, aim for 0.50) so the system cannot oscillate a pair back and forth burning fees. The property test "no policy setting can cause fee spend without net imbalance reduction over a simulated day" guards this.

### 5.4 Multi-asset rule

Channels carry native CKB or a UDT (`funding_udt_type_script`). A circular payment is denominated in one asset; **a rebalance pairs channels of the same asset only.** The planner partitions channels by asset before step 1. Cross-asset liquidity shaping (via swaps) is explicitly out of scope — that's a trading system with price risk, not a rebalancer.

---

## 6. The Executor state machine

This is where money bugs live, so it's specified precisely. One action at a time; each action is a row in `rebalance_actions` moving through:

```
 PLANNED ──dry_run ok──▶ PRICED ──approved──▶ SUBMITTING ──rpc accepted──▶ CONFIRMING ──Success──▶ SETTLED
    │                      │        (advisory: operator click;                 │
    │                      │         autopilot: policy auto)                   ├──Failed──▶ FAILED(reason)
    └──no route/太expensive──▶ REJECTED(reason)                                └──timeout──▶ STUCK → reconcile
```

Rules:

- **PRICED → SUBMITTING** re-checks the budget ledger *at execution time* (the price was from planning time; the daily ledger may have moved).
- **SUBMITTING** writes the action row with a client-generated `intent_id` *before* the RPC call, then records `payment_hash` from the response.
- **The crash window:** a crash after `send_payment` returns but before `payment_hash` is persisted leaves an in-flight payment we don't have a handle to. Recovery (§7) reconciles via `list_payments` filtered to the reconcile window, matching on amount + self-target + timestamp. This window is honest, small, and bounded — documented rather than hidden.
- **CONFIRMING** polls `get_payment` with backoff. A payment neither settling nor failing past the TLC expiry horizon goes to **STUCK**, which blocks new submissions (the money may still move) and alerts the operator. Stuck is expected to be rare and self-resolving at TLC expiry; the state exists so the tool *never runs concurrently with its own unresolved action*.
- Terminal states record `actual_fee` (from `get_payment`), which is what enters the daily fee ledger — not the dry-run estimate.

Startup sequence: load policy → reconcile any non-terminal actions (§7) → resume ticking. The loop never plans while any action is non-terminal.

---

## 7. Failure modes and recovery

| Failure | Effect | Handling |
|---|---|---|
| FNN RPC unreachable | No fresh snapshots | Degrade to read-only: dashboard shows last-known + staleness banner; metrics gauge `evenkeel_rpc_up 0`; no planning on stale data older than policy limit |
| Crash mid-SUBMITTING | Possible in-flight payment without recorded hash | On startup: `list_payments` over reconcile window, match by (self-target, amount, created_at ≈ intent time); adopt or mark ORPHAN-SUSPECT and alert |
| Crash in CONFIRMING | Action row has payment_hash | `get_payment(hash)` on startup → drive to terminal state |
| Payment STUCK in-flight | Liquidity temporarily locked | Block new actions; alert; TLC expiry resolves it; then reconcile |
| DB lost (snapshots) | History gone | Snapshots rebuild from live polling within minutes; drift detection blind for one window. Acceptable |
| DB lost (action log / fee ledger) | Audit + today's budget accounting gone | Fee ledger conservatively reseeds from `list_payments` (self-payments today); audit loss is real and documented — operators who care mount the volume |
| Node restarts / channels close mid-plan | Intent references dead channel | Executor re-validates channel `state == ChannelReady` immediately before SUBMITTING; else REJECTED(stale) |
| Testnet route sparsity | Circular routes unfindable | Not a runtime failure — a scoping gate (§11 kill criterion): advisory-only mode remains fully functional |

Worst-case economic loss under any combination of the above: the daily fee budget. That bound is the safety story, and it's enforced twice (our ledger, node's `max_fee_amount`).

---

## 8. Testing strategy (the part that makes this buildable solo)

The central constraint: **a live, dense payment-channel network cannot be assumed** — testnet may be sparse, and CI certainly has no network. So the architecture makes the network optional:

1. **Property tests on `evenkeel-core`** (proptest):
   - Planner never proposes amount > usable surplus/deficit; never pairs cross-asset; never exceeds any budget.
   - Classification is monotone in ratio; hysteresis prevents re-trigger within cooldown.
   - Simulated-day invariant: for any policy, total fees spent ≤ daily cap AND net imbalance does not increase.
2. **`MockNode` scenario tests** on the full loop: scripted drift patterns (steady drain, burst traffic, oscillating), fault injection (dry_run says yes then send fails; payment sticks; RPC times out), asserting the state machine lands in correct terminal states and the ledger is exact.
3. **Deterministic simulation harness**: replay a synthetic 24h traffic pattern against MockNode; output balance-ratio trajectories with/without Even Keel. **This doubles as the demo fallback** — if testnet cooperates, demo live; if not, demo the simulation with real code paths and say so plainly (the hackathon brief explicitly rewards declaring what's real vs simulated).
4. **Testnet E2E** as a smoke test only, never CI: one scripted run proving a real circular rebalance settles.

The mock being a trait implementation (not HTTP stubs) keeps tests fast and the domain honest: if the core needs something the trait doesn't expose, that's a design signal, not a mocking chore.

---

## 9. Security posture and trust boundaries

```
[Dashboard browser] ──HTTPS/basic-auth──▶ [evenkeel-server] ──localhost/docker net──▶ [FNN RPC]
                                                 │
                                                 └──▶ [Postgres]   (same trust zone)
```

- The FNN RPC is the crown jewels (it can spend). Even Keel deploys **co-located** with the node (same host / same Docker network); the RPC port is never exposed beyond that boundary, per FNN's own docs.
- Even Keel's own API exposes read-only monitoring plus rebalance approval. v1 auth: bind to localhost or LAN + basic auth; anything internet-facing is the operator's reverse-proxy decision, documented, not owned.
- No keys are held. The blast radius of a full Even Keel compromise = the node RPC exposure it already had, plus fee-budget drain — bounded again by the node-side `max_fee_amount` on each call.
- Every automated action is in the audit log with the policy snapshot that authorized it.

---

## 10. Observability (of the tool itself)

Prometheus at `/metrics` — the tool that watches channels must itself be watchable:

```
evenkeel_channel_usable_ratio{channel,asset}      gauge
evenkeel_channels_by_state{state}                 gauge
evenkeel_drift_slope{channel}                     gauge
evenkeel_rebalance_actions_total{result,mode}     counter
evenkeel_rebalance_fee_shannons_total             counter
evenkeel_fee_budget_remaining_shannons            gauge
evenkeel_rpc_up / evenkeel_snapshot_age_seconds   gauges
evenkeel_action_state{state}                      gauge   (non-terminal action visibility)
```

Grafana dashboard JSON ships in `ops/`. Structured `tracing` throughout with `intent_id` as the correlation field from plan to settlement.

---

## 11. Delivery plan (hackathon window) and the gate

**Prep week (before 1 July) — the gate:** stand up FNN on Pudge testnet, open 2–3 channels, execute one manual `dry_run` then real circular self-payment.
- **Green:** full plan below.
- **Red (routes unfindable):** pivot to advisory + simulation demo. Written down now so it's a decision, not a scramble.

**Days 1–4 — spine:** `FiberRpc` trait + Real/Mock impls; poller; snapshots; health engine with property tests; `/metrics`; dashboard skeleton with channel cards + drift charts.
**Days 5–9 — money path:** planner; executor state machine with reconciliation; advisory flow end-to-end (propose → show dry-run fee → click → settle → log). MockNode scenario suite green.
**Days 10–12 — autopilot + simulation:** policy engine, opt-in autopilot with budgets; 24h simulation harness producing the with/without chart.
**Days 13–14 — ship:** hosted demo, video, README (including the required "gap addressed" section — the FNN TODO line), docs, submission checklist against all nine graded deliverables.

---

## 12. Post-hackathon roadmap (the "continue after" story)

1. **Passive rebalancing via fee policy** — `update_channel(tlc_fee_proportional_millionths)`: cheapen saturated channels to attract draining flow. The elegant complement to active rebalancing (charge-lnd's insight); v2 headline.
2. **Smarter planning** — min-cost-flow global planner when node channel counts justify it; route learning from failure history.
3. **Fleet mode** — one Even Keel, many nodes (schema already keys by node).
4. **LSP primitives** — liquidity quotes and readiness endpoints, aligning with the brief's LSP direction and a Community Fund DAO grant path.
5. **Cross-network awareness** — Fiber↔Lightning via CCH once stable; strictly after single-network correctness is boring.

---

## Appendix A — Architecture Decision Records

**ADR-1: External tool over node fork.** A fork inherits the node's release cadence, audit burden, and merge conflicts forever; the RPC already exposes everything needed. Rejected: patching FNN. Consequence: we live within the RPC's capabilities — acceptable, they're sufficient (§2).

**ADR-2: One serialized control loop; at most one in-flight action.** Concurrent rebalances share route liquidity, race the fee ledger, and invalidate each other's snapshots. At operator scale (≤ dozens of channels, actions costing seconds), throughput is irrelevant and explainability is everything. Rejected: per-channel-pair concurrency. Consequence: a stuck action blocks the queue — mitigated by the STUCK state + alerting rather than by concurrency.

**ADR-3: Greedy pairwise planner, not min-cost flow.** Global optimization is the "correct" formulation and the wrong v1: harder to test, harder to explain to an operator ("why did it move that?"), and worth single-digit % over greedy at small n. Rejected for v1, kept in the roadmap with the trigger condition (n large or measured regret). Consequence: mildly suboptimal fee efficiency, majorly superior explainability.

**ADR-4: Advisory default, autopilot opt-in.** A tool that spends money by default is wrong for first contact regardless of how good the bounds are. The demo shows autopilot; the default config ships advisory. Rejected: autopilot-first for demo impact — the brief rewards production judgment over spectacle.

**ADR-5: Postgres time-series (SQLite option), not in-memory.** Drift detection and the fee ledger need durable history; operators need restarts to be boring. Rejected: in-memory with periodic dump (loses the ledger exactly when it matters).

**ADR-6: Trait-mocked node as a first-class artifact.** The mock is the development environment, the CI environment, and the demo fallback. This is the single decision that makes a solo two-week build survivable against an immature testnet. Rejected: testing against testnet only (hostage to route sparsity and faucet economics).

**ADR-7: Integer money everywhere.** `u128` Shannons; basis-point integer ratios in decision paths; floats for display only. Rejected: f64 convenience — the class of bug it invites is silent and financial.