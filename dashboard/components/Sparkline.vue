<script setup lang="ts">
// Usable-ratio drift sparkline: single series (no legend — the card names it),
// fixed 0–100% domain so cards compare, recessive threshold guides at the
// 20%/80% classification bounds, crosshair + tooltip on hover.
import type { HistoryPoint } from '~/types/api'
import { formatBp, formatTime } from '~/utils/format'

const props = defineProps<{ history: HistoryPoint[] }>()

const W = 280
const H = 64

const points = computed(() =>
  props.history.filter((p): p is HistoryPoint & { usable_ratio_bp: number } => p.usable_ratio_bp != null),
)

const coords = computed(() => {
  const pts = points.value
  if (pts.length < 2) return []
  const t0 = pts[0]!.at_ms
  const t1 = pts[pts.length - 1]!.at_ms
  const span = Math.max(t1 - t0, 1)
  return pts.map((p) => ({
    x: ((p.at_ms - t0) / span) * W,
    y: H - (p.usable_ratio_bp / 10_000) * H,
    p,
  }))
})

const linePath = computed(() =>
  coords.value.map((c, i) => `${i === 0 ? 'M' : 'L'}${c.x.toFixed(1)},${c.y.toFixed(1)}`).join(' '),
)

const yAt = (bp: number) => H - (bp / 10_000) * H

const hover = ref<number | null>(null)
const onMove = (e: MouseEvent) => {
  const el = e.currentTarget as HTMLElement
  const rect = el.getBoundingClientRect()
  const x = ((e.clientX - rect.left) / rect.width) * W
  if (!coords.value.length) return
  let best = 0
  for (let i = 1; i < coords.value.length; i++) {
    if (Math.abs(coords.value[i]!.x - x) < Math.abs(coords.value[best]!.x - x)) best = i
  }
  hover.value = best
}
const hovered = computed(() => (hover.value == null ? null : coords.value[hover.value] ?? null))
</script>

<template>
  <div class="spark" @mousemove="onMove" @mouseleave="hover = null">
    <svg v-if="coords.length >= 2" :viewBox="`0 0 ${W} ${H}`" preserveAspectRatio="none">
      <!-- Classification bounds, recessive -->
      <line :x1="0" :x2="W" :y1="yAt(2000)" :y2="yAt(2000)" class="guide" />
      <line :x1="0" :x2="W" :y1="yAt(8000)" :y2="yAt(8000)" class="guide" />
      <path :d="linePath" class="line" />
      <template v-if="hovered">
        <line :x1="hovered.x" :x2="hovered.x" :y1="0" :y2="H" class="crosshair" />
        <circle :cx="hovered.x" :cy="hovered.y" r="3.5" class="dot" />
      </template>
    </svg>
    <p v-else class="empty">collecting history…</p>
    <div
      v-if="hovered"
      class="tooltip mono"
      :style="{
        left: `${(hovered.x / W) * 100}%`,
        transform: hovered.x < W / 2 ? 'translateX(10px)' : 'translateX(calc(-100% - 10px))',
      }"
    >
      {{ formatBp(hovered.p.usable_ratio_bp) }} · {{ formatTime(hovered.p.at_ms) }}
    </div>
  </div>
</template>

<style scoped>
.spark {
  position: relative;
  height: 64px;
}
svg {
  width: 100%;
  height: 100%;
  display: block;
  overflow: visible;
}
.line {
  fill: none;
  stroke: var(--series-1);
  stroke-width: 2;
  stroke-linejoin: round;
  stroke-linecap: round;
  vector-effect: non-scaling-stroke;
}
.guide {
  stroke: var(--border);
  stroke-width: 1;
  stroke-dasharray: 3 4;
  vector-effect: non-scaling-stroke;
}
.crosshair {
  stroke: var(--text-muted);
  stroke-width: 1;
  vector-effect: non-scaling-stroke;
}
.dot {
  fill: var(--series-1);
  stroke: var(--surface-1);
  stroke-width: 2;
}
.empty {
  margin: 0;
  height: 100%;
  display: grid;
  place-items: center;
  color: var(--text-muted);
  font-size: 12px;
}
.tooltip {
  position: absolute;
  top: -6px;
  padding: 2px 8px;
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 11px;
  color: var(--text-secondary);
  white-space: nowrap;
  pointer-events: none;
}
</style>
