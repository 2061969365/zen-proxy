# Build stage
FROM rust:1.80-slim-bookworm AS builder

WORKDIR /usr/src/zen-proxy

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml ./
COPY src ./src

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/zen-proxy/target/release/zen-proxy /app/zen-proxy

ENV PORT=4096 \
    HOST=0.0.0.0 \
    RUST_LOG=info

EXPOSE 4096

CMD ["/app/zen-proxy"]
