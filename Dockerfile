# syntax=docker/dockerfile:1.7

FROM rust:1.98.0-bookworm AS builder
ARG CARGO_BUILD_JOBS=2
WORKDIR /workspace
COPY . .
RUN --mount=type=cache,id=silicon-iam-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=silicon-briefcase-target,target=/workspace/target,sharing=locked \
    cargo build --locked --release --bins --jobs "$CARGO_BUILD_JOBS" \
    && install -D -m 0755 target/release/briefcase-api /opt/silicon-briefcase/briefcase-api \
    && install -D -m 0755 target/release/briefcase-worker /opt/silicon-briefcase/briefcase-worker \
    && install -D -m 0755 target/release/briefcase-migrate /opt/silicon-briefcase/briefcase-migrate

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 briefcase \
    && useradd --system --uid 10001 --gid briefcase --no-create-home --home-dir /nonexistent briefcase

COPY --from=builder /opt/silicon-briefcase/briefcase-api /usr/local/bin/briefcase-api
COPY --from=builder /opt/silicon-briefcase/briefcase-worker /usr/local/bin/briefcase-worker
COPY --from=builder /opt/silicon-briefcase/briefcase-migrate /usr/local/bin/briefcase-migrate

USER 10001:10001

ENV RUST_BACKTRACE=0

EXPOSE 8080
STOPSIGNAL SIGTERM

# The image carries all three processes and the command selects one, so a
# deployment runs `briefcase-worker` or `briefcase-migrate` from the same
# immutable image it serves the API from. An ENTRYPOINT would swallow that
# choice and pass the name to the API as an argument instead.
CMD ["briefcase-api"]
