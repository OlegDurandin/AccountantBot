# syntax=docker/dockerfile:1.7

FROM rust:1.95-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN --mount=type=cache,id=accountant-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=accountant-cargo-git,target=/usr/local/cargo/git/db,sharing=locked \
    --mount=type=cache,id=accountant-cargo-target,target=/app/target,sharing=locked \
    CARGO_INCREMENTAL=0 cargo build --locked --release \
    && cp /app/target/release/accountant-bot /tmp/accountant-bot

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates fontconfig fonts-dejavu-core gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 bot
WORKDIR /app
COPY --from=builder /tmp/accountant-bot /usr/local/bin/accountant-bot
COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN mkdir -p /app/data && chown -R bot:bot /app
RUN chmod 0755 /usr/local/bin/docker-entrypoint.sh
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["accountant-bot"]
