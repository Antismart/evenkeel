<script setup lang="ts">
// §7 degrade-to-read-only surface: when the node RPC is down or snapshots are
// old, the operator must know the picture is stale before trusting it.
import { formatTime } from '~/utils/format'

defineProps<{
  rpcUp: boolean
  stale: boolean
  lastSnapshotMs: number | null
  serverUnreachable: boolean
}>()
</script>

<template>
  <div v-if="serverUnreachable" class="banner critical" role="alert">
    <strong>dashboard offline</strong> — evenkeel-server unreachable; showing the last loaded picture.
  </div>
  <div v-else-if="!rpcUp || stale" class="banner warning" role="alert">
    <strong>stale data</strong> — node RPC {{ rpcUp ? 'reachable' : 'unreachable' }};
    last snapshot {{ formatTime(lastSnapshotMs) }}. No decisions are made on stale data.
  </div>
</template>

<style scoped>
.banner {
  padding: 10px 16px;
  border-radius: 10px;
  font-size: 13px;
  border: 1px solid;
  margin-bottom: 20px;
}
.banner.warning {
  color: var(--status-warning);
  border-color: color-mix(in srgb, var(--status-warning) 45%, transparent);
  background: color-mix(in srgb, var(--status-warning) 8%, transparent);
}
.banner.critical {
  color: var(--status-critical);
  border-color: color-mix(in srgb, var(--status-critical) 45%, transparent);
  background: color-mix(in srgb, var(--status-critical) 8%, transparent);
}
</style>
