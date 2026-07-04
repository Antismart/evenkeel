// Polls the evenkeel-server dashboard payload. The server maintains the
// cache; the client just refreshes it — an outage shows up as `stale` /
// `rpc_up: false` in the payload, not as a broken page.

import type { Dashboard } from '~/types/api'

const POLL_MS = 5_000

export function useDashboard() {
  const data = ref<Dashboard | null>(null)
  const error = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null

  const refresh = async () => {
    try {
      data.value = await $fetch<Dashboard>('/api/channels')
      error.value = null
    } catch (e) {
      // Server itself unreachable (worse than node-down): keep last data,
      // surface the reason — the banner switches to "dashboard offline".
      error.value = e instanceof Error ? e.message : String(e)
    }
  }

  onMounted(() => {
    refresh()
    timer = setInterval(refresh, POLL_MS)
  })
  onUnmounted(() => {
    if (timer) clearInterval(timer)
  })

  return { data, error, refresh }
}
