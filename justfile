default: check

check: fmt-check clippy test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

build:
    cargo build --workspace --release

run *ARGS:
    cargo run -p spacehog -- {{ARGS}}

bench-home:
    cargo build --release -p spacehog && time ./target/release/spacehog scan "$HOME" --json

gif:
    vhs docs/demo.tape
