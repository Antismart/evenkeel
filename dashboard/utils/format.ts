// Display-only formatting (ADR-7: floats never feed decisions — these strings
// end at the DOM). Balances arrive as u128 Shannon decimal strings; BigInt
// keeps full precision until the final rounding for display.

const SHANNONS_PER_CKB = 100_000_000n

/** "35100000000" Shannons → "351.00" CKB (two decimals, exact until display). */
export function formatCkb(shannons: string): string {
  let v: bigint
  try {
    v = BigInt(shannons)
  } catch {
    return '—'
  }
  const whole = v / SHANNONS_PER_CKB
  // Two decimal places, rounded down — an operator reading balances wants
  // truthful floor, not optimistic rounding.
  const cents = ((v % SHANNONS_PER_CKB) * 100n) / SHANNONS_PER_CKB
  return `${whole.toLocaleString()}.${cents.toString().padStart(2, '0')}`
}

/** 5333 bp → "53.3%". */
export function formatBp(bp: number | null): string {
  if (bp == null) return '—'
  return `${(bp / 100).toFixed(1)}%`
}

/** Drift slope → signed "+3.2 %/h". */
export function formatDrift(bpPerHour: number | null): string {
  if (bpPerHour == null) return '—'
  const pct = bpPerHour / 100
  const sign = pct > 0 ? '+' : ''
  return `${sign}${pct.toFixed(1)} %/h`
}

/** Shorten a pubkey/hash for display: 0x1234…abcd. */
export function shorten(hex: string, head = 10, tail = 4): string {
  if (hex.length <= head + tail + 1) return hex
  return `${hex.slice(0, head)}…${hex.slice(-tail)}`
}

/** ms-epoch → "12:34:56" local time. */
export function formatTime(ms: number | null): string {
  if (ms == null) return 'never'
  return new Date(ms).toLocaleTimeString()
}
