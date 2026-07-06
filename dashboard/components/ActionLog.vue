<script setup lang="ts">
// The audit trail: every action with its state, fees, and reason. States are
// labeled text with a status dot — never color alone.
import type { ActionState, ActionView } from '~/types/api'
import { formatCkb, formatTime, shorten } from '~/utils/format'

defineProps<{ actions: ActionView[] }>()

const stateRole = (s: ActionState): string =>
  ({
    settled: 'good',
    priced: 'info',
    submitting: 'warning',
    confirming: 'warning',
    stuck: 'critical',
    failed: 'critical',
    orphan_suspect: 'critical',
    rejected: 'muted',
    planned: 'muted',
  })[s] ?? 'muted'

const feeOf = (a: ActionView): string => {
  if (a.actual_fee) return `${formatCkb(a.actual_fee)}`
  if (a.quoted_fee) return `${formatCkb(a.quoted_fee)} (quote)`
  return '—'
}
</script>

<template>
  <section aria-label="action log">
    <h2>Action log</h2>
    <p v-if="!actions.length" class="empty">No rebalance actions yet.</p>
    <div v-else class="scroll">
      <table class="mono">
        <thead>
          <tr>
            <th>time</th>
            <th>route</th>
            <th class="num">amount CKB</th>
            <th class="num">fee CKB</th>
            <th>state</th>
            <th>detail</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="a in actions" :key="a.intent_id">
            <td>{{ formatTime(a.created_at_ms) }}</td>
            <td>{{ shorten(a.source_channel, 10, 4) }} → {{ shorten(a.sink_channel, 10, 4) }}</td>
            <td class="num">{{ formatCkb(a.amount) }}</td>
            <td class="num">{{ feeOf(a) }}</td>
            <td>
              <span class="state" :data-role="stateRole(a.state)">
                <span class="dot" aria-hidden="true" />{{ a.state }}
              </span>
            </td>
            <td class="detail">{{ a.reason ?? (a.payment_hash ? shorten(a.payment_hash, 10, 4) : '') }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </section>
</template>

<style scoped>
h2 {
  font-size: 15px;
  margin: 28px 0 12px;
}
.empty {
  color: var(--text-muted);
  font-size: 13px;
}
.scroll {
  overflow-x: auto;
  border: 1px solid var(--border);
  border-radius: 10px;
}
table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
th,
td {
  text-align: left;
  padding: 8px 12px;
  white-space: nowrap;
}
th {
  color: var(--text-muted);
  font-weight: 500;
  border-bottom: 1px solid var(--border);
  background: var(--surface-1);
}
tbody tr:nth-child(odd) {
  background: color-mix(in srgb, var(--surface-1) 55%, transparent);
}
.num {
  text-align: right;
}
.state {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--text-muted);
}
.state[data-role='good'] .dot { background: var(--status-good); }
.state[data-role='info'] .dot { background: var(--series-1); }
.state[data-role='warning'] .dot { background: var(--status-warning); }
.state[data-role='critical'] .dot { background: var(--status-critical); }
.detail {
  color: var(--text-muted);
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
