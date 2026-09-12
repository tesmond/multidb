.PHONY: build install dev check test

build:
	@cargo build --manifest-path desktop/Cargo.toml --release
	@if [ "$$(uname -s)" = "Darwin" ]; then sh build/darwin/bundle.sh; fi

install:
	@cargo fetch --manifest-path desktop/Cargo.toml

dev:
	@cargo run --manifest-path desktop/Cargo.toml

check:
	@cargo check --quiet --manifest-path desktop/Cargo.toml

test:
	@cargo test --quiet --manifest-path desktop/Cargo.toml
