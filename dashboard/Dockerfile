# Dashboard: Nuxt build + Nitro runtime. The API base is baked at build time
# (Nitro routeRules are compiled), so compose passes it as a build arg.
FROM node:22-alpine AS builder
WORKDIR /app
RUN corepack enable
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
ARG NUXT_PUBLIC_API_BASE_URL=http://evenkeel-server:3030
ENV NUXT_PUBLIC_API_BASE_URL=$NUXT_PUBLIC_API_BASE_URL
RUN pnpm build

FROM node:22-alpine
WORKDIR /app
COPY --from=builder /app/.output ./.output
EXPOSE 3000
ENV PORT=3000 HOST=0.0.0.0
CMD ["node", ".output/server/index.mjs"]
