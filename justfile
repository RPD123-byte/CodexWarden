set dotenv-load := false

build:
    cargo build --workspace --all-targets

test:
    cargo test --workspace

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

schema-regen:
    ./scripts/regenerate-schema.sh

check: build test lint
