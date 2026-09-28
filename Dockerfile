FROM rust:1.83-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && curl -sL "https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2Fv5.6.0/kustomize_v5.6.0_linux_$(dpkg --print-architecture).tar.gz" | tar xz -C /usr/local/bin
COPY --from=build /src/target/release/igniteflux /usr/local/bin/igniteflux
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/igniteflux"]
