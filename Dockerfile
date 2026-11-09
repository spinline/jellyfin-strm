# Multi-stage Dockerfile for jellyfin-strm (Linux ARM64 / x86_64)
FROM rust:1-bookworm AS builder

WORKDIR /src

# Cache dependencies
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy real source code and build
COPY src src
RUN touch src/main.rs && cargo build --release

# Final runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    tzdata \
    curl \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/jellyfin-strm /usr/local/bin/jellyfin-strm

ENV RUST_LOG=info
ENV OUTPUT_DIR=/media

VOLUME ["/media"]

ENTRYPOINT ["/usr/local/bin/jellyfin-strm"]
