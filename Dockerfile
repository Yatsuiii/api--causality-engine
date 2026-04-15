# ── Planner stage ─────────────────────────────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1 AS planner
WORKDIR /build
COPY . .
RUN cargo chef prepare --recipe-path recipe.json --package ace

# ── Builder stage ─────────────────────────────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1 AS builder
WORKDIR /build
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --package ace --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ace /usr/local/bin/ace

WORKDIR /scenarios

ENTRYPOINT ["ace"]
CMD ["--help"]
