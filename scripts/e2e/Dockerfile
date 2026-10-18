# Linux peer for the end-to-end test: real X11 clipboard (Xvfb) + xclip.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends xvfb xclip \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/clipboard-unifier /usr/local/bin/clipboard-unifier
COPY scripts/e2e/entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
