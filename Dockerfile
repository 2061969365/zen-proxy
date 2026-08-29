FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

COPY zen-proxy-linux-amd64 /app/zen-proxy
RUN chmod +x /app/zen-proxy

ENV PORT=4096 \
    HOST=0.0.0.0 \
    RUST_LOG=info

EXPOSE 4096

CMD ["/app/zen-proxy"]
