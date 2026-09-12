.PHONY: build dev check test fetch bundle

# Extra cargo flags, e.g. `make build CARGO_FLAGS=--no-default-features` to
# compile the Metal shaders ahead of time (needs Xcode, not just the Command
# Line Tools).
CARGO_FLAGS ?=
CARGO = cargo $(1) --manifest-path desktop/Cargo.toml $(CARGO_FLAGS)

build:
	@$(call CARGO,build) --release
	@if [ "$$(uname -s)" = "Darwin" ]; then sh build/darwin/bundle.sh; fi

bundle:
	@sh build/darwin/bundle.sh

dev:
	@$(call CARGO,run)

check:
	@$(call CARGO,check) --quiet

test:
	@$(call CARGO,test) --quiet

fetch:
	@$(call CARGO,fetch)
