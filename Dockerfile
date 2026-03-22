# Build stage
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release --features webhook,sqs,redis,postgres,nats,dashboard

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    libssl3 \
    libpq5 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/claude-channel-server /usr/local/bin/claude-channel-server
COPY --from=builder /app/target/release/claude-channel-shim /usr/local/bin/claude-channel-shim
# Keep legacy binaries for backward compat
COPY --from=builder /app/target/release/claude-channel /usr/local/bin/claude-channel
COPY --from=builder /app/target/release/claude-channel-dashboard /usr/local/bin/claude-channel-dashboard

EXPOSE 9000 8900

CMD ["claude-channel-server"]
