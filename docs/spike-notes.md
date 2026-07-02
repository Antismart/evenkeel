# Phase 0 spike notes — FNN Pudge testnet gate

**Gate status: GREEN** — a real circular self-payment settled on Pudge testnet.
**Date run:** 2026-07-02/03 (UTC)
**FNN version:** v0.8.1, built from source into `fiber:0.8.1-local` (no 0.8.x image is published on Docker Hub/GHCR). RPC per `crates/fiber-lib/src/rpc/README.md` at tag v0.8.1.
**Operator:** Antismart

## Environment

- macOS host, Docker Desktop; node via `ops/spike/setup-fnn.sh` + `docker-compose.yml`.
- RPC on `127.0.0.1:8227` (host-loopback). Note: v0.8.x refuses "public" RPC binds
  (including `0.0.0.0`) without biscuit auth; RFC1918 addresses pass `is_public_addr()`,
  so the container binds a static `172.28.0.2` and Docker maps it loopback-only.
- Node pubkey (`node_info`): `0285605841146b278eb2f5ed817ceeb1810924a3fb1ca8f5ac4abe6fef43203cef`
- Funding: faucet.nervos.org → 100,000 CKB in one cell to
  `ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqtvfy2cqndwjeuq7qd5gtgyt54q0xelepqv3y92j`

## Channels opened

All CKB-denominated, 500 CKB funding each (→ 401 CKB usable local after the ~99 CKB
reserve/occupancy), all reached `CHANNEL_READY`:

| # | Peer | channel_id | Notes |
|---|---|---|---|
| 1 | `024714ca…` bootnodesgp | `0xda091458e6dd8d9c…` | testnet bootnode; already peered |
| 2 | `03d8c8d0…` (43.198.254.225 cluster) | `0xd9580c67ad2c3cfa…` | connected to us inbound |
| 3 | `024714ca…` bootnodesgp (parallel) | `0xe6a950b634e50ce0…` | second channel to same peer |

**Topology decision:** the public graph (51 nodes / 261 channels at the time) is
fragmented into many small components; most well-connected nodes announce
unreachable private IPs (`172.31.x`), and the two best multi-node circle candidates
(CkbaNode-1, `0280a4…@52.76.106.120`) accept TCP but never complete the Fiber `Init`
handshake, so `open_channel` fails with "feature not found, waiting for peer to send
Init message". Pivoted to **two parallel channels to the same reliable peer**
(bootnodesgp) — a 2-hop circle `us → bootnodesgp → us` that exercises the identical
`send_payment(allow_self_payment)` primitive.

**Inbound liquidity:** fresh channels carry zero remote balance, so no return leg
exists until the counterparty holds funds toward us. Fixed by a direct keysend push
of 150 CKB to bootnodesgp (fee 0, settled instantly), which landed on channel #3's
remote side.

## Gate test 1 — dry_run

`send_payment` params (via `./fnn-rpc.sh dry-run --amount 10000000000 --max-fee 100000000`):

```json
[{
  "target_pubkey": "0285605841146b278eb2f5ed817ceeb1810924a3fb1ca8f5ac4abe6fef43203cef",
  "amount": "0x2540be400",
  "keysend": true,
  "allow_self_payment": true,
  "max_fee_amount": "0x5f5e100",
  "dry_run": true
}]
```

Response: `status: "Created"`, `fee: 0x989680` (10,000,000 Shannons = 0.1 CKB —
the default 0.1% `tlc_fee_proportional_millionths`). Route found on the first try;
no `build_router` fallback needed.

## Gate test 2 — real circular self-payment

`./fnn-rpc.sh rebalance --amount 10000000000 --max-fee 100000000` (same params,
`dry_run: false` — the script enforces dry-run-before-send on identical params):

- Dry-run quoted fee: 10,000,000 Shannons
- payment_hash: `0x5888472682b1b6dbc954c625ae44dd2ff4d1a57a64985d6b3832352d3684cc54`
- Status: `Created → Inflight → Success` in ~6 s of polling
- Actual fee (`get_payment`): 10,000,000 Shannons — matched the quote exactly

Balances (CKB):

| channel | local before | local after | delta |
|---|---|---|---|
| `0xda091458…` (out leg) | 401 | 300.9 | −100.1 (amount + fee) |
| `0xe6a950b6…` (return leg) | 251 | 351 | +100 |
| `0xd9580c67…` (uninvolved) | 401 | 401 | 0 |

Net node balance change: −0.1 CKB = exactly the routing fee. Principal preserved.

## Failures / retries / surprises

1. **No 0.8.x Docker image published** — built `fiber:0.8.1-local` from source
   (`setup-fnn.sh` automates this).
2. **RPC bind restriction** — `0.0.0.0` counts as public and requires biscuit auth;
   fixed with a static private container IP.
3. **Funding race on a single wallet cell** — opening two channels concurrently made
   both funding txs compete for the one faucet cell; the loser aborted
   ("Funding transaction aborted" / "capacity not enough"). Fix: open channels
   sequentially, waiting for the change cell to confirm. **Even Keel's executor
   serialization (ADR-2) is validated by the node's own behavior here.**
4. **Peers that never complete Init** — CkbaNode-1 and both `52.76.106.120` nodes
   accept TCP but drop before the Fiber handshake; `open_channel` to them fails.
   Testnet peer quality is the dominant friction, exactly as §11 anticipated.
5. **Stale/private address announcements** — 16 nodes announce the same
   `43.198.254.225:8226` multiaddr with different peer IDs; top-degree nodes announce
   AWS-internal `172.31.x` addresses.
6. **Inbound liquidity bootstrap** — expected and confirmed: a fresh channel cannot be
   a return leg; keysend push works as the bootstrap.

## Verdict

- [x] **GREEN** — real circular self-payment settled; proceed with the full plan.
- [ ] ~~RED~~

Implications carried into Phase 1+: circular self-payment works end-to-end on
v0.8.1 including dry-run pricing (fee quote == actual fee in our run); route
sparsity is real, so the MockNode-first strategy (ADR-6) and the §5.3 planner's
route-capacity estimate matter; the 2-hop parallel-channel circle is a valid
degenerate case the planner should support.
