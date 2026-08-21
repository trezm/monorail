# Build context is the repository root (the web service sets
# RAILWAY_DOCKERFILE_PATH=ui/Dockerfile), so the pnpm workspace resolves.
FROM node:24-slim AS build
WORKDIR /src
RUN npm install -g pnpm@11.20.0
COPY package.json pnpm-workspace.yaml pnpm-lock.yaml ./
COPY ui/package.json ui/
RUN pnpm install --frozen-lockfile
COPY ui ui
# Baked into the static bundle; Railway passes the service variable as a
# build arg. Empty fails the build in astro.config.mjs, by design.
ARG PUBLIC_API_URL
ENV PUBLIC_API_URL=${PUBLIC_API_URL}
RUN pnpm --filter ui build

FROM caddy:2-alpine
COPY ui/Caddyfile /etc/caddy/Caddyfile
COPY --from=build /src/ui/dist /srv
