# syntax=docker/dockerfile:1.7

FROM rust:1.98-bookworm AS builder
WORKDIR /workspace
COPY . .
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home briefcase
COPY --from=builder /workspace/target/release/briefcase-api /usr/local/bin/
COPY --from=builder /workspace/target/release/briefcase-worker /usr/local/bin/
COPY --from=builder /workspace/target/release/briefcase-migrate /usr/local/bin/
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["briefcase-api"]
