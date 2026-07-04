<script setup lang="ts">
// Health class chip: status color + text label + direction glyph — meaning
// never rides on color alone.
import type { HealthClass } from '~/types/api'

const props = defineProps<{ health: HealthClass }>()

const meta = computed(() => {
  switch (props.health) {
    case 'depleted':
      return { label: 'depleted', glyph: '▼', role: 'critical' }
    case 'depleting':
      return { label: 'depleting', glyph: '↓', role: 'warning' }
    case 'filling':
      return { label: 'filling', glyph: '↑', role: 'warning' }
    case 'saturated':
      return { label: 'saturated', glyph: '▲', role: 'serious' }
    default:
      return { label: 'healthy', glyph: '●', role: 'good' }
  }
})
</script>

<template>
  <span class="badge" :data-role="meta.role">
    <span aria-hidden="true">{{ meta.glyph }}</span>
    {{ meta.label }}
  </span>
</template>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 2px 10px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
  border: 1px solid var(--border);
  color: var(--text-secondary);
}
.badge[data-role='good'] {
  color: var(--status-good);
  border-color: color-mix(in srgb, var(--status-good) 40%, transparent);
}
.badge[data-role='warning'] {
  color: var(--status-warning);
  border-color: color-mix(in srgb, var(--status-warning) 40%, transparent);
}
.badge[data-role='serious'] {
  color: var(--status-serious);
  border-color: color-mix(in srgb, var(--status-serious) 40%, transparent);
}
.badge[data-role='critical'] {
  color: var(--status-critical);
  border-color: color-mix(in srgb, var(--status-critical) 40%, transparent);
}
</style>
