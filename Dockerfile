# syntax=docker/dockerfile:1
FROM rust:1.99.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --locked --release --bin wikikiki

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 wikikiki \
    && useradd --uid 10001 --gid 10001 --home-dir /data --no-create-home --shell /usr/sbin/nologin wikikiki \
    && mkdir -p /data /etc/wikikiki \
    && chown 10001:10001 /data
COPY --from=build /build/target/release/wikikiki /usr/local/bin/wikikiki
COPY wikikiki.example.toml /etc/wikikiki/wikikiki.toml
ENV WIKIKIKI_BIND=0.0.0.0:8090 HOME=/data
WORKDIR /data
USER 10001:10001
VOLUME ["/data"]
EXPOSE 8090
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent --show-error --max-time 4 http://127.0.0.1:8090/healthz > /dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/wikikiki", "--config", "/etc/wikikiki/wikikiki.toml"]
CMD ["serve"]
