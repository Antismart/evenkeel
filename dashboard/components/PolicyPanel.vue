<script setup lang="ts">
// Policy summary + the autopilot switch. Autopilot is opt-in (ADR-4): the
// toggle spells out exactly what flipping it means, and the change round-
// trips through PUT /api/policy so the persisted flag survives restarts.
import { formatCkb } from '~/utils/format'

interface PolicyBody {
  target_ratio_bp: number
  max_amount_per_action: string
  max_fee_per_action: string
  max_fee_per_day: string
  min_benefit_bp_per_ckb_fee: number
  cooldown_ticks: number
  depleted_below_bp: number
  saturated_above_bp: number
  drift_bp_per_hour: number
  min_drift_points: number
  autopilot: boolean
}

const policy = ref<PolicyBody | null>(null)
const busy = ref(false)
const error = ref<string | null>(null)

const load = async () => {
  try {
    policy.value = await $fetch<PolicyBody>('/api/policy')
    error.value = null
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
}
onMounted(load)

const toggleAutopilot = async () => {
  if (!policy.value || busy.value) return
  busy.value = true
  try {
    policy.value = await $fetch<PolicyBody>('/api/policy', {
      method: 'PUT',
      body: { ...policy.value, autopilot: !policy.value.autopilot },
    })
    error.value = null
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section v-if="policy" class="panel" aria-label="policy">
    <div class="row">
      <div class="autopilot">
        <button
          class="switch"
          role="switch"
          :aria-checked="policy.autopilot"
          :disabled="busy"
          @click="toggleAutopilot"
        >
          <span class="knob" :data-on="policy.autopilot" />
        </button>
        <div>
          <p class="label">
            Autopilot
            <strong :data-on="policy.autopilot">{{ policy.autopilot ? 'ON' : 'OFF' }}</strong>
          </p>
          <p class="hint">
            {{ policy.autopilot
              ? 'Spending within budget without approval — priced rebalances execute on the next tick.'
              : 'Advisory: every rebalance waits for your approval.' }}
          </p>
        </div>
      </div>
      <dl class="bounds mono">
        <div><dt>max/action</dt><dd>{{ formatCkb(policy.max_fee_per_action) }} CKB fee</dd></div>
        <div><dt>daily budget</dt><dd>{{ formatCkb(policy.max_fee_per_day) }} CKB</dd></div>
        <div><dt>target</dt><dd>{{ (policy.target_ratio_bp / 100).toFixed(0) }}%</dd></div>
        <div><dt>cooldown</dt><dd>{{ policy.cooldown_ticks }} ticks</dd></div>
      </dl>
    </div>
    <p v-if="error" class="error">policy update failed: {{ error }}</p>
  </section>
</template>

<style scoped>
.panel {
  background: var(--surface-1);
  border: 1px solid var(--border);
  border-radius: 12px;
  padding: 14px 16px;
  margin-bottom: 20px;
}
.row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 20px;
  flex-wrap: wrap;
}
.autopilot {
  display: flex;
  align-items: center;
  gap: 12px;
}
.switch {
  width: 44px;
  height: 24px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: var(--surface-2);
  position: relative;
  cursor: pointer;
  padding: 0;
}
.switch:disabled {
  opacity: 0.5;
  cursor: wait;
}
.knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--text-muted);
  transition: transform 0.15s ease, background 0.15s ease;
}
.knob[data-on='true'] {
  transform: translateX(20px);
  background: var(--status-good);
}
.label {
  margin: 0;
  font-size: 13px;
  font-weight: 600;
}
.label strong[data-on='true'] {
  color: var(--status-good);
}
.label strong[data-on='false'] {
  color: var(--text-muted);
}
.hint {
  margin: 2px 0 0;
  font-size: 12px;
  color: var(--text-muted);
  max-width: 420px;
}
.bounds {
  margin: 0;
  display: flex;
  gap: 20px;
  flex-wrap: wrap;
  font-size: 12px;
}
.bounds div {
  display: grid;
  gap: 2px;
}
dt {
  color: var(--text-muted);
  font-size: 11px;
}
dd {
  margin: 0;
  color: var(--text-secondary);
}
.error {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--status-critical);
}
</style>
