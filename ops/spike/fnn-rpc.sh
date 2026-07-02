#!/usr/bin/env bash
# Phase 0 spike: manual FNN JSON-RPC driver for the gate test.
#
# Commands:
#   ./fnn-rpc.sh info                          node_info (shows own pubkey)
#   ./fnn-rpc.sh channels                      list_channels, balances decoded,
#                                              usable_out/in per architecture §5.1
#   ./fnn-rpc.sh dry-run   --amount N [--max-fee N]
#                                              price a circular self-payment
#                                              (dry_run: true — moves nothing)
#   ./fnn-rpc.sh rebalance --amount N --max-fee N [--yes]
#                                              dry-run first, show the fee,
#                                              confirm, send for real, then
#                                              poll get_payment to settlement
#   ./fnn-rpc.sh payment <payment_hash>        get_payment
#   ./fnn-rpc.sh payments                      list_payments
#   ./fnn-rpc.sh raw <method> [params-json]    escape hatch, e.g.
#                                              ./fnn-rpc.sh raw graph_nodes '[{"limit":"0x14"}]'
#
# Amounts are DECIMAL Shannons on the command line (1 CKB = 100_000_000
# Shannons); the script hex-encodes them for the wire as FNN requires.
# CKB-denominated channels only — pass UDT payments via `raw` if needed.
#
# Invariant (CLAUDE.md rule 6): no real send without a preceding dry_run on the
# SAME parameters. `rebalance` enforces this by construction — the dry-run and
# the real send are built from one shared params object, differing only in the
# dry_run flag. `max_fee_amount` rides on both, so the node enforces the fee
# ceiling even if this script is wrong.
#
# Env: FNN_RPC_URL (default http://127.0.0.1:8227)
set -euo pipefail

FNN_RPC_URL="${FNN_RPC_URL:-http://127.0.0.1:8227}"

# jq helper: FNN serializes u128 as 0x-hex strings. Decoding to jq numbers is
# exact only below 2^53 Shannons (~90M CKB) — far above any spike channel, and
# these decodes are display-only anyway.
JQ_HEXDEC='def hexdec:
  if . == null then null
  else ltrimstr("0x") | ascii_downcase | explode
       | reduce .[] as $c (0; . * 16 + (if $c >= 97 then $c - 87 else $c - 48 end))
  end;'

die() { echo "error: $*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }
need curl; need jq

# rpc <method> [params-json-array]  → prints .result, dies on JSON-RPC error
rpc() {
  local method=$1 params=${2:-[]} resp
  resp=$(curl -sS -X POST "$FNN_RPC_URL" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}") \
    || die "RPC transport failure against $FNN_RPC_URL (is the node up?)"
  if jq -e 'has("error")' >/dev/null <<<"$resp"; then
    echo "RPC error from $method:" >&2
    jq '.error' <<<"$resp" >&2
    return 1
  fi
  jq '.result' <<<"$resp"
}

dec2hex() {
  [[ "$1" =~ ^[0-9]+$ ]] || die "amount must be a decimal integer (Shannons), got: $1"
  printf '0x%x' "$1"
}

self_pubkey() {
  rpc node_info | jq -r '.pubkey' | grep -E '^[0-9a-f]{66}$' \
    || die "could not read own pubkey from node_info"
}

# Build the send_payment params object shared by dry-run and real send.
# $1 = amount hex, $2 = max_fee hex or empty, $3 = pubkey, $4 = dry_run bool
send_params() {
  local amount_hex=$1 max_fee_hex=$2 pubkey=$3 dry=$4
  jq -n --arg pk "$pubkey" --arg amt "$amount_hex" --arg fee "$max_fee_hex" --argjson dry "$dry" '
    [{
      target_pubkey: $pk,
      amount: $amt,
      keysend: true,
      allow_self_payment: true,
      dry_run: $dry
    } + (if $fee == "" then {} else {max_fee_amount: $fee} end)]'
}

cmd_info() { rpc node_info | jq '{version, pubkey, node_name, addresses, channel_count, peers_count}'; }

cmd_channels() {
  rpc list_channels '[{}]' | jq "$JQ_HEXDEC"'
    .channels[] | {
      channel_id,
      peer: .pubkey,
      state: (.state.state_name // .state),
      enabled,
      asset: (if .funding_udt_type_script then "UDT" else "CKB" end),
      local_balance: (.local_balance | hexdec),
      remote_balance: (.remote_balance | hexdec),
      offered_tlc_balance: (.offered_tlc_balance | hexdec),
      received_tlc_balance: (.received_tlc_balance | hexdec)
    }
    | .usable_out = .local_balance - .offered_tlc_balance
    | .usable_in  = .remote_balance - .received_tlc_balance
    | .capacity   = .local_balance + .remote_balance
    | .local_ratio_pct = (if .capacity > 0 then (10000 * .local_balance / .capacity | floor) / 100 else null end)'
}

cmd_payment()  { rpc get_payment "[{\"payment_hash\":\"$1\"}]"; }
cmd_payments() { rpc list_payments '[{}]'; }

# Parse --amount / --max-fee / --yes
parse_flags() {
  AMOUNT="" MAX_FEE="" YES=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --amount)  AMOUNT=$2; shift 2 ;;
      --max-fee) MAX_FEE=$2; shift 2 ;;
      --yes)     YES=1; shift ;;
      *) die "unknown flag: $1" ;;
    esac
  done
}

run_dry() { # $1 amount_hex, $2 max_fee_hex, $3 pubkey → prints dry-run result
  local out
  out=$(rpc send_payment "$(send_params "$1" "$2" "$3" true)") || return 1
  echo "$out"
}

cmd_dry_run() {
  parse_flags "$@"
  [ -n "$AMOUNT" ] || die "--amount is required"
  local pk amount_hex max_fee_hex="" out
  pk=$(self_pubkey)
  amount_hex=$(dec2hex "$AMOUNT")
  [ -n "$MAX_FEE" ] && max_fee_hex=$(dec2hex "$MAX_FEE")
  echo "dry_run circular self-payment: amount=${AMOUNT} Shannons, target=self (${pk})"
  out=$(run_dry "$amount_hex" "$max_fee_hex" "$pk") || die "dry-run failed — no circular route at this amount (this is the RED signal if it persists across amounts/channels)"
  jq "$JQ_HEXDEC"'{status, fee_shannons: (.fee | hexdec), routers}' <<<"$out"
}

cmd_rebalance() {
  parse_flags "$@"
  [ -n "$AMOUNT" ]  || die "--amount is required"
  [ -n "$MAX_FEE" ] || die "--max-fee is required for a real send (node-side fee ceiling)"
  local pk amount_hex max_fee_hex dry_out fee send_out phash status
  pk=$(self_pubkey)
  amount_hex=$(dec2hex "$AMOUNT")
  max_fee_hex=$(dec2hex "$MAX_FEE")

  echo "== Step 1/3: dry_run (pricing before commitment) =="
  dry_out=$(run_dry "$amount_hex" "$max_fee_hex" "$pk") || die "dry-run failed; not sending"
  fee=$(jq -r "$JQ_HEXDEC"'.fee | hexdec' <<<"$dry_out")
  echo "quoted fee: ${fee} Shannons (max allowed: ${MAX_FEE})"
  jq "$JQ_HEXDEC"'{status, routers}' <<<"$dry_out"

  if [ "$YES" -ne 1 ]; then
    printf 'Send for real? [y/N] '
    read -r reply
    [ "$reply" = "y" ] || [ "$reply" = "Y" ] || { echo "aborted"; exit 0; }
  fi

  echo "== Step 2/3: send_payment (same params, dry_run: false) =="
  send_out=$(rpc send_payment "$(send_params "$amount_hex" "$max_fee_hex" "$pk" false)") \
    || die "send failed after successful dry-run — record this in spike-notes"
  phash=$(jq -r '.payment_hash' <<<"$send_out")
  echo "payment_hash: $phash"

  echo "== Step 3/3: polling get_payment until terminal =="
  for _ in $(seq 1 60); do
    status=$(rpc get_payment "[{\"payment_hash\":\"$phash\"}]" | jq -r '.status')
    echo "  status: $status"
    case "$status" in
      Success)
        rpc get_payment "[{\"payment_hash\":\"$phash\"}]" \
          | jq "$JQ_HEXDEC"'{payment_hash, status, actual_fee_shannons: (.fee | hexdec)}'
        echo "SETTLED. Run './fnn-rpc.sh channels' to see the shifted balances."
        exit 0 ;;
      Failed)
        rpc get_payment "[{\"payment_hash\":\"$phash\"}]" | jq '{payment_hash, status, failed_error}'
        echo "FAILED — principal unmoved (benign failure mode, architecture §4)."
        exit 1 ;;
    esac
    sleep 2
  done
  echo "still non-terminal after 120s — payment_hash $phash; keep polling with:"
  echo "  ./fnn-rpc.sh payment $phash"
  exit 1
}

case "${1:-}" in
  info)      shift; cmd_info "$@" ;;
  channels)  shift; cmd_channels "$@" ;;
  dry-run)   shift; cmd_dry_run "$@" ;;
  rebalance) shift; cmd_rebalance "$@" ;;
  payment)   shift; [ $# -ge 1 ] || die "usage: fnn-rpc.sh payment <payment_hash>"; cmd_payment "$@" ;;
  payments)  shift; cmd_payments "$@" ;;
  raw)       shift; [ $# -ge 1 ] || die "usage: fnn-rpc.sh raw <method> [params-json]"; rpc "$@" ;;
  *) grep '^#' "$0" | sed 's/^# \{0,1\}//' | sed -n '2,30p'; exit 1 ;;
esac
