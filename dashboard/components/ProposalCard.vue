<script setup lang="ts">
// The advisory heart of the tool: a priced rebalance proposal with its exact
// dry-run fee, waiting for the operator's click. Nothing moves without it.
import type { ActionView } from '~/types/api'
import { formatCkb, shorten } from '~/utils/format'

const props = defineProps<{ action: ActionView }>()
const emit = defineEmits<{ decided: [] }>()

const busy = ref(false)

const decide = async (verb: 'approve' | 'reject') => {
  if (busy.value) return
  busy.value = true
  try {
    await $fetch(`/api/actions/${props.action.intent_id}/${verb}`, { method: 'POST' })
    emit('decided')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section class="proposal" aria-label="proposed rebalance">
    <header>
      <h2>Proposed rebalance</h2>
      <span class="mode">{{ action.mode }}</span>
    </header>
    <p class="pair mono">
      {{ shorten(action.source_channel, 12, 4) }}
      <span class="arrow" aria-hidden="true">→</span>
      {{ shorten(action.sink_channel, 12, 4) }}
    </p>
    <dl class="mono">
      <div><dt>amount</dt><dd>{{ formatCkb(action.amount) }} CKB</dd></div>
      <div><dt>quoted fee</dt><dd>{{ action.quoted_fee ? formatCkb(action.quoted_fee) : '—' }} CKB</dd></div>
      <div><dt>imbalance reduced</dt><dd>{{ (action.benefit_bp / 100).toFixed(2) }} pp</dd></div>
    </dl>
    <p class="note">
      Priced via <code>dry_run</code> just now; the node re-enforces this fee ceiling on send.
      A failed rebalance costs nothing.
    </p>
    <div class="buttons">
      <button class="approve" :disabled="busy" @click="decide('approve')">Approve &amp; send</button>
      <button class="reject" :disabled="busy" @click="decide('reject')">Reject</button>
    </div>
  </section>
</template>

<style scoped>
.proposal {
  background: var(--surface-1);
  border: 1px solid color-mix(in srgb, var(--series-1) 45%, transparent);
  border-radius: 12px;
  padding: 18px;
  margin-bottom: 20px;
  display: grid;
  gap: 10px;
}
header {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
}
h2 {
  margin: 0;
  font-size: 15px;
}
.mode {
  font-size: 11px;
  color: var(--text-muted);
  border: 1px solid var(--border);
  border-radius: 999px;
  padding: 1px 8px;
}
.pair {
  margin: 0;
  font-size: 14px;
  color: var(--text-secondary);
}
.arrow {
  color: var(--series-1);
  margin: 0 6px;
}
dl {
  margin: 0;
  display: flex;
  gap: 28px;
  flex-wrap: wrap;
  font-size: 13px;
}
dl div {
  display: grid;
  gap: 2px;
}
dt {
  color: var(--text-muted);
  font-size: 11px;
}
dd {
  margin: 0;
  color: var(--text-primary);
  font-weight: 600;
}
.note {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}
.buttons {
  display: flex;
  gap: 10px;
}
button {
  font: inherit;
  font-size: 13px;
  font-weight: 600;
  padding: 8px 16px;
  border-radius: 8px;
  border: 1px solid var(--border);
  background: var(--surface-2);
  color: var(--text-primary);
  cursor: pointer;
}
button:disabled {
  opacity: 0.5;
  cursor: wait;
}
.approve {
  background: color-mix(in srgb, var(--status-good) 18%, var(--surface-2));
  border-color: color-mix(in srgb, var(--status-good) 45%, transparent);
}
.reject:hover {
  border-color: color-mix(in srgb, var(--status-critical) 45%, transparent);
}
.approve:hover:not(:disabled) {
  background: color-mix(in srgb, var(--status-good) 28%, var(--surface-2));
}
</style>
