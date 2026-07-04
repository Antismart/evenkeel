<script setup lang="ts">
// One channel: identity, health class, usable-ratio meter, balances, drift
// sparkline. Everything numeric is preformatted display text (ADR-7).
import type { ChannelView } from '~/types/api'
import { formatBp, formatCkb, formatDrift, shorten } from '~/utils/format'

const props = defineProps<{ channel: ChannelView }>()

const ratioPct = computed(() => {
  const bp = props.channel.health.usable_ratio_bp
  return bp == null ? 0 : bp / 100
})
const assetLabel = computed(() => (props.channel.asset === 'ckb' ? 'CKB' : 'UDT'))
</script>

<template>
  <article class="card">
    <header>
      <div>
        <h3 class="mono">{{ shorten(channel.channel_id, 12, 6) }}</h3>
        <p class="peer mono">peer {{ shorten(channel.peer, 8, 4) }} · {{ assetLabel }}</p>
      </div>
      <HealthBadge :health="channel.health.class" />
    </header>

    <div class="ratio">
      <div class="meter" role="meter" :aria-valuenow="ratioPct" aria-valuemin="0" aria-valuemax="100"
        :aria-label="`usable outbound ${formatBp(channel.health.usable_ratio_bp)}`">
        <div class="fill" :style="{ width: `${ratioPct}%` }" />
        <span class="bound lo" />
        <span class="bound hi" />
      </div>
      <p class="ratio-row">
        <span class="value mono">{{ formatBp(channel.health.usable_ratio_bp) }}</span>
        <span class="label">usable outbound</span>
        <span class="drift mono">{{ formatDrift(channel.health.drift_bp_per_hour) }}</span>
      </p>
    </div>

    <Sparkline :history="channel.history" />

    <dl class="balances mono">
      <div><dt>local</dt><dd>{{ formatCkb(channel.local_balance) }}</dd></div>
      <div><dt>usable out</dt><dd>{{ formatCkb(channel.usable_out) }}</dd></div>
      <div><dt>remote</dt><dd>{{ formatCkb(channel.remote_balance) }}</dd></div>
      <div><dt>usable in</dt><dd>{{ formatCkb(channel.usable_in) }}</dd></div>
    </dl>
  </article>
</template>

<style scoped>
.card {
  background: var(--surface-1);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 16px;
  display: grid;
  gap: 14px;
}
header {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: 12px;
}
h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}
.peer {
  margin: 2px 0 0;
  font-size: 11px;
  color: var(--text-muted);
}
.meter {
  position: relative;
  height: 8px;
  border-radius: 4px;
  background: var(--surface-2);
  overflow: hidden;
}
.fill {
  height: 100%;
  border-radius: 4px;
  background: var(--series-1);
  transition: width 0.4s ease;
}
.bound {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 1px;
  background: var(--border);
}
.bound.lo { left: 20%; }
.bound.hi { left: 80%; }
.ratio-row {
  margin: 6px 0 0;
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-size: 12px;
  color: var(--text-muted);
}
.value {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
}
.drift {
  margin-left: auto;
  color: var(--text-secondary);
}
.balances {
  margin: 0;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px 16px;
  font-size: 12px;
}
.balances div {
  display: flex;
  justify-content: space-between;
  gap: 8px;
}
dt {
  color: var(--text-muted);
}
dd {
  margin: 0;
  color: var(--text-secondary);
}
</style>
