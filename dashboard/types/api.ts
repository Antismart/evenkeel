// Mirrors evenkeel-server/src/state.rs. Balances are u128 Shannons as decimal
// strings — JSON numbers cannot carry them; format with BigInt, display-only.

export type HealthClass = 'depleted' | 'depleting' | 'healthy' | 'filling' | 'saturated'

export interface ChannelHealth {
  channel_id: string
  class: HealthClass
  usable_ratio_bp: number | null
  drift_bp_per_hour: number | null
}

export interface HistoryPoint {
  at_ms: number
  usable_ratio_bp: number | null
}

export interface ChannelView {
  channel_id: string
  peer: string
  asset: string
  health: ChannelHealth
  local_balance: string
  remote_balance: string
  usable_out: string
  usable_in: string
  history: HistoryPoint[]
}

export interface NodeStatus {
  node_pubkey: string
  node_version: string
  rpc_up: boolean
  last_snapshot_ms: number | null
  stale: boolean
}

export type ActionState =
  | 'planned'
  | 'priced'
  | 'submitting'
  | 'confirming'
  | 'settled'
  | 'failed'
  | 'rejected'
  | 'stuck'
  | 'orphan_suspect'

export interface ActionView {
  intent_id: string
  asset: string
  source_channel: string
  sink_channel: string
  amount: string
  benefit_bp: number
  state: ActionState
  mode: string
  quoted_fee: string | null
  actual_fee: string | null
  payment_hash: string | null
  reason: string | null
  created_at_ms: number
  updated_at_ms: number
}

export interface LedgerView {
  spent_today: string
  daily_budget: string
}

export interface Dashboard {
  status: NodeStatus
  channels: ChannelView[]
  actions: ActionView[]
  ledger: LedgerView
}
