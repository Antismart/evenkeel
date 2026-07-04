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

export interface Dashboard {
  status: NodeStatus
  channels: ChannelView[]
}
