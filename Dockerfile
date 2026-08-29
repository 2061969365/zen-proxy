# Build stage
FROM rust:1.80-alpine AS builder

WORKDIR /usr/src/zen-proxy

# Install build dependencies for Alpine
RUN apk add --no-cache musl-dev

# Copy manifests
COPY Cargo.toml ./

# Copy source code
COPY src ./src

# Build release binary
RUN cargo build --release

# Final runtime stage
FROM alpine:3.20

WORKDIR /app

RUN apk add --no-cache ca-certificates tzdata

COPY --from=builder /usr/src/zen-proxy/target/release/zen-proxy /app/zen-proxy

ENV PORT=4096 \
    HOST=0.0.0.0 \
    RUST_LOG=info

EXPOSE 4096

CMD ["/app/zen-proxy"]
