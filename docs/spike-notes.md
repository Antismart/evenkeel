# Phase 0 spike notes — FNN Pudge testnet gate

**Gate status: PENDING** <!-- change to GREEN or RED; CLAUDE.md blocks Phase 1 until this is set -->
**Date run:**
**FNN version:** `nervos/fiber:v0.8.1` (docker), RPC per `crates/fiber-lib/src/rpc/README.md` at tag v0.8.1
**Operator:**

## Environment

- Host / OS:
- Node started via `ops/spike/setup-fnn.sh` + `docker-compose.yml` (RPC on `127.0.0.1:8227`, host-local)
- Node pubkey (`node_info`):
- Funding address + faucet amounts received:

## Channels opened

| # | Peer (pubkey / alias) | channel_id | Funding (Shannons) | Notes (how peer was chosen, time to CHANNEL_READY) |
|---|---|---|---|---|
| 1 | | | | |
| 2 | | | | |
| 3 | | | | |

Inbound-liquidity situation on the return path (what, if anything, was needed
to make a circular route possible):

## Gate test 1 — dry_run

Command: `./fnn-rpc.sh dry-run --amount <N>`

Exact `send_payment` params sent:

```json
```

Response (status, quoted fee, routers):

```json
```

## Gate test 2 — real circular self-payment

Command: `./fnn-rpc.sh rebalance --amount <N> --max-fee <N>`

- Dry-run quoted fee (Shannons):
- payment_hash:
- Final status (`get_payment`):
- Actual fee (Shannons):
- Time to settle:

Balances (from `./fnn-rpc.sh channels`):

| channel_id | local before | local after | delta |
|---|---|---|---|
| | | | |
| | | | |

## Failures / retries / surprises

<!-- every "no route", param that behaved unlike the docs, amount that had to shrink, etc. -->

## Verdict

- [ ] **GREEN** — real circular self-payment settled; proceed with the full plan.
- [ ] **RED** — no circular route findable after trying: smaller amounts, all channel pairs,
  and explicit routing via `build_router` + `send_payment_with_router`.
  → Pivot per CLAUDE.md: advisory-only; Phases 1–2 unchanged; Phase 3 autopilot replaced by
  deeper simulation demo. **Update CLAUDE.md before writing any code.**
