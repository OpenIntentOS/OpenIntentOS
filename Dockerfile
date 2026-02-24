# =============================================================================
# OpenIntentOS - Multi-stage Docker Build
# =============================================================================
# Produces a minimal static binary (~< 20MB final image)
#
# Build:  docker build -t openintentos .
# Run:    docker run -p 3000:3000 -e ANTHROPIC_API_KEY=sk-ant-... openintentos
# =============================================================================

# ---------------------------------------------------------------------------
# Stage 1: Build the static binary
# ---------------------------------------------------------------------------
FROM rust:1.84-slim AS builder

# Install musl toolchain for fully static linking
RUN apt-get update && \
    apt-get install -y --no-install-recommends musl-tools pkg-config && \
    rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /src

# Cache dependencies: copy manifests first, build a dummy, then copy real source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/

# Build the release binary with static musl linking
RUN cargo build \
    --release \
    --target x86_64-unknown-linux-musl \
    --bin openintent

# ---------------------------------------------------------------------------
# Stage 2: Minimal runtime image
# ---------------------------------------------------------------------------
FROM alpine:3.20

# ca-certificates for TLS connections to Anthropic API
RUN apk add --no-cache ca-certificates && \
    addgroup -S openintent && \
    adduser -S openintent -G openintent

# Copy the statically-linked binary
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/openintent /usr/local/bin/openintent

# Copy configuration files
COPY config/ /etc/openintent/config/

# Create data directory for persistent storage
RUN mkdir -p /data && chown openintent:openintent /data
VOLUME /data

USER openintent
WORKDIR /data

EXPOSE 3000

ENTRYPOINT ["openintent", "serve"]
