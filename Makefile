.PHONY: build install dev check

build:
	@bun run --silent --cwd frontend build
	@cargo build --manifest-path desktop/Cargo.toml --release
	@if [ "$$(uname -s)" = "Darwin" ]; then sh build/darwin/bundle.sh; fi

install:
	@bun install

dev:
	@bun run --silent --cwd frontend build
	@cargo run --manifest-path desktop/Cargo.toml

check:
	@bun run --silent --cwd frontend check
	@cargo check --quiet --manifest-path desktop/Cargo.toml
