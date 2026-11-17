# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p squad-rendezvous

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --shell /usr/sbin/nologin squad

COPY --from=builder /app/target/release/squad-rendezvous /usr/local/bin/squad-rendezvous

USER squad

EXPOSE 3478/udp

ENTRYPOINT ["squad-rendezvous"]
CMD ["--bind", "0.0.0.0:3478"]
