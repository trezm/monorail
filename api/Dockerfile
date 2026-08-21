# Build context is the repository root (the api service sets
# RAILWAY_DOCKERFILE_PATH=api/Dockerfile), so the workspace manifests resolve.
FROM rust:1.95-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY api api
RUN cargo build --release --locked --package monorail-api --bins

FROM debian:trixie-slim
WORKDIR /app
COPY --from=build /src/target/release/api /src/target/release/migrate ./
USER nobody
# Migrations run as the pre-deploy command (see railway.json), one job per
# deploy, so replicas never race them at startup.
CMD ["./api"]
