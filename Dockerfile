# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.87-slim AS builder

WORKDIR /build

# Cache dependencies first
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml    crates/core/Cargo.toml
COPY crates/runner/Cargo.toml  crates/runner/Cargo.toml
COPY crates/http/Cargo.toml    crates/http/Cargo.toml
COPY crates/model/Cargo.toml   crates/model/Cargo.toml
COPY crates/cli/Cargo.toml     crates/cli/Cargo.toml

# Dummy src files so cargo can resolve the workspace
RUN mkdir -p crates/core/src crates/runner/src crates/http/src crates/model/src crates/cli/src && \
    echo "fn main() {}" > crates/cli/src/main.rs && \
    for c in core runner http model; do echo "" > crates/$c/src/lib.rs; done && \
    cargo build --package ace --release && \
    rm -rf crates/*/src

# Real build
COPY crates crates
RUN touch crates/cli/src/main.rs && \
    cargo build --package ace --release

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/ace /usr/local/bin/ace

WORKDIR /scenarios

ENTRYPOINT ["ace"]
CMD ["--help"]
