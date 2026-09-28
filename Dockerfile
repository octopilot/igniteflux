# Build stage: BuildKit cache mounts (host paths are not visible to the daemon in GitHub Actions)
FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    env CARGO_TARGET_DIR=/app/target cargo build --release \
    && cp /app/target/release/igniteflux /app/igniteflux

# Run stage: git for the checkout, kustomize for the render
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && curl -sL "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.6.0/kustomize_v5.6.0_linux_$(dpkg --print-architecture).tar.gz" | tar xz -C /usr/local/bin \
 && mkdir -p /var/lib/igniteflux && chown 65532:65532 /var/lib/igniteflux
COPY --from=builder /app/igniteflux /usr/local/bin/igniteflux
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/igniteflux"]
