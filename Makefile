.PHONY: build dev check test test-db test-db-down fetch bundle release release-dry-run

# Extra cargo flags, e.g. `make build CARGO_FLAGS=--no-default-features` to
# compile the Metal shaders ahead of time (needs Xcode, not just the Command
# Line Tools).
CARGO_FLAGS ?=
CARGO = cargo $(1) $(CARGO_FLAGS)

build:
	@$(call CARGO,build) --release
	@if [ "$$(uname -s)" = "Darwin" ]; then sh packaging/macos/bundle.sh; fi

bundle:
	@sh packaging/macos/bundle.sh

# `make release VERSION=1.2.3`: set Cargo.toml's version to VERSION, commit
# and push the bump, build the macOS app and publish GitHub release v<VERSION>
# with a changelog of commits since the last tag. Needs GITHUB_TOKEN (or `gh`
# login). See packaging/release.sh.
VERSION ?=
release:
	@[ -n "$(VERSION)" ] || { echo "usage: make release VERSION=x.y.z" >&2; exit 1; }
	@CARGO_FLAGS="$(CARGO_FLAGS)" sh packaging/release.sh "$(VERSION)"

# Same bump, build and changelog, but the bump is reverted afterwards and
# nothing is committed, pushed or uploaded.
release-dry-run:
	@[ -n "$(VERSION)" ] || { echo "usage: make release-dry-run VERSION=x.y.z" >&2; exit 1; }
	@CARGO_FLAGS="$(CARGO_FLAGS)" DRY_RUN=1 sh packaging/release.sh "$(VERSION)"

dev:
	@$(call CARGO,run)

check:
	@$(call CARGO,check) --quiet

test:
	@$(call CARGO,test) --quiet

# The result-editing tests against real Postgres and MySQL servers (Docker).
# `make test` runs the same tests against SQLite only.
TEST_PG ?= postgres://postgres:multidb@127.0.0.1:55432/multidb_test
TEST_MYSQL ?= mysql://root:multidb@127.0.0.1:53306/multidb_test
test-db:
	@docker compose -f docker-compose.test.yml up -d --wait
	@MULTIDB_TEST_PG=$(TEST_PG) MULTIDB_TEST_MYSQL=$(TEST_MYSQL) $(call CARGO,test) --lib grid_edit

test-db-down:
	@docker compose -f docker-compose.test.yml down -v

fetch:
	@$(call CARGO,fetch)
