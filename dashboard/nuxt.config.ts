// Single source of truth for the API origin (cellora-nuxt pattern). In dev the
// Nitro proxy forwards same-origin /api and /metrics to the evenkeel-server so
// app code uses relative URLs; in Docker the env var points at the server
// container.
const apiBaseUrl = process.env.NUXT_PUBLIC_API_BASE_URL || 'http://127.0.0.1:3030'

export default defineNuxtConfig({
  compatibilityDate: '2026-07-01',
  // Operator dashboard: pure client-side app; the Rust server is the only backend.
  ssr: false,
  devtools: { enabled: false },
  runtimeConfig: {
    public: { apiBaseUrl },
  },
  nitro: {
    routeRules: {
      '/api/**': { proxy: `${apiBaseUrl}/api/**` },
    },
  },
  app: {
    head: {
      title: 'Even Keel — Fiber channel liquidity',
      htmlAttrs: { lang: 'en' },
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      ],
      link: [
        { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' },
        {
          rel: 'stylesheet',
          href: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500;600&display=swap',
        },
      ],
    },
  },
  css: ['~/assets/css/main.css'],
  components: [{ path: '~/components', pathPrefix: false }],
})
