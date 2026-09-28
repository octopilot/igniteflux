# igniteflux — development task runner (https://github.com/casey/just)

default:
    @just --list

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all-features

build:
    cargo build --release

image tag="dev":
    docker build -t ghcr.io/octopilot/igniteflux:{{tag}} .

chart-lint:
    cd chart/chartTemplate && helm lint .

install-hooks:
    pre-commit install

pre-commit:
    pre-commit run --all-files

# Run against the current kubeconfig context with a config file (needs POD_NAMESPACE for the App secret lookup)
run config="config/example.yaml" ns="crossplane-system":
    IGNITEFLUX_CONFIG={{config}} POD_NAMESPACE={{ns}} RUST_LOG=info cargo run
