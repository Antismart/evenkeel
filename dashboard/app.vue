<script setup lang="ts">
import { shorten } from '~/utils/format'

const { data, error } = useDashboard()

const channels = computed(() => data.value?.channels ?? [])
const status = computed(() => data.value?.status ?? null)
</script>

<template>
  <div class="page">
    <header class="top">
      <div class="brand">
        <span class="keel" aria-hidden="true">⚖</span>
        <div>
          <h1>Even Keel</h1>
          <p class="sub">Fiber channel liquidity</p>
        </div>
      </div>
      <p v-if="status" class="node mono">
        {{ status.node_version || '—' }} ·
        {{ status.node_pubkey ? shorten(status.node_pubkey, 10, 6) : 'connecting…' }}
      </p>
    </header>

    <StalenessBanner
      :rpc-up="status?.rpc_up ?? false"
      :stale="status?.stale ?? true"
      :last-snapshot-ms="status?.last_snapshot_ms ?? null"
      :server-unreachable="error != null"
    />

    <main>
      <div v-if="channels.length" class="grid">
        <ChannelCard v-for="ch in channels" :key="ch.channel_id" :channel="ch" />
      </div>
      <p v-else-if="data" class="empty">No ready channels yet — open one and it appears here.</p>
      <p v-else class="empty">Loading…</p>
    </main>
  </div>
</template>

<style scoped>
.page {
  max-width: 1100px;
  margin: 0 auto;
  padding: 28px 20px 60px;
}
.top {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 24px;
  gap: 16px;
  flex-wrap: wrap;
}
.brand {
  display: flex;
  align-items: center;
  gap: 12px;
}
.keel {
  font-size: 28px;
}
h1 {
  margin: 0;
  font-size: 20px;
  letter-spacing: -0.01em;
}
.sub {
  margin: 0;
  font-size: 12px;
  color: var(--text-muted);
}
.node {
  font-size: 12px;
  color: var(--text-secondary);
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 16px;
}
.empty {
  color: var(--text-muted);
  text-align: center;
  padding: 60px 0;
}
</style>
