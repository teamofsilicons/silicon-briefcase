# syntax=docker/dockerfile:1.7

FROM rust:1.98-bookworm AS builder
WORKDIR /workspace
COPY . .
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 briefcase \
    && useradd --system --uid 10001 --gid briefcase --no-create-home --home-dir /nonexistent briefcase

COPY --from=builder /workspace/target/release/briefcase-api /usr/local/bin/briefcase-api
COPY --from=builder /workspace/target/release/briefcase-worker /usr/local/bin/briefcase-worker
COPY --from=builder /workspace/target/release/briefcase-migrate /usr/local/bin/briefcase-migrate

USER 10001:10001

ENV RUST_BACKTRACE=0

EXPOSE 8080
STOPSIGNAL SIGTERM

# The image carries all three processes and the command selects one, so a
# deployment runs `briefcase-worker` or `briefcase-migrate` from the same
# immutable image it serves the API from. An ENTRYPOINT would swallow that
# choice and pass the name to the API as an argument instead.
CMD ["briefcase-api"]
