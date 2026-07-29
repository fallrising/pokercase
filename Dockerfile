# thinrouter / pokercase — multi-stage build
FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY templates ./templates
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/thinrouter /usr/local/bin/thinrouter
ENV THINROUTER_HOST=0.0.0.0
ENV THINROUTER_PORT=20128
ENV THINROUTER_DATA_DIR=/data
EXPOSE 20128
VOLUME ["/data"]
ENTRYPOINT ["thinrouter"]
CMD ["serve"]
