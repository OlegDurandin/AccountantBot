FROM rust:1.95-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates fontconfig fonts-dejavu-core gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot
WORKDIR /app
COPY --from=builder /app/target/release/accountant-bot /usr/local/bin/accountant-bot
COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN mkdir -p /app/data && chown -R bot:bot /app
RUN chmod 0755 /usr/local/bin/docker-entrypoint.sh
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["accountant-bot"]
