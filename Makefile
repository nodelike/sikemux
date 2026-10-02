.PHONY: dev dev-stop showcase build e2e preflight run icons format format-check lint test test-coverage tsc rust-fmt rust-clippy rust-test rust-audit shell-lint release-check hooks prepush check ci clean clean-dev prune

# Homebrew's Rust ignores rust-toolchain.toml, so rustup's proxies must come first.
export PATH := $(HOME)/.cargo/bin:$(PATH)

icons:
	./scripts/icons.sh

dev: export BITBUCKET_OAUTH_SECRET ?= $(shell sed -n 's/^BITBUCKET_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: icons
	@node scripts/prune-target.mjs --daily >/dev/null 2>&1 &
	pnpm dev:desktop

# Stops the dev build's core and the terminals and agents it runs.
dev-stop:
	@test -x src-tauri/target/debug/sikemux-editor || { echo "No dev build yet; run make dev first."; exit 1; }
	src-tauri/target/debug/sikemux-editor core stop

showcase:
	pnpm showcase:serve --open /showcase/

# Release artifacts target the host architecture (Apple Silicon on the
# supported release machine).
build: icons
	./scripts/build-mac.sh

# Launches the real app the way CI's desktop E2E job does.
e2e:
	pnpm test:e2e:desktop --browser

# Before tagging a release: the checks only the Release workflow would otherwise reach.
preflight: e2e build

run:
	./src-tauri/target/release/sikemux

format:
	pnpm format
	pnpm rust:fmt

format-check:
	pnpm format:check
	pnpm rust:fmt:check

lint:
	pnpm lint

test:
	pnpm test

test-coverage:
	pnpm test:coverage

tsc:
	pnpm typecheck

rust-fmt:
	pnpm rust:fmt:check

rust-clippy:
	pnpm rust:clippy

rust-test:
	pnpm rust:test

rust-audit:
	pnpm audit:rust

shell-lint:
	pnpm shell:lint

release-check:
	pnpm release:check

hooks:
	git config core.hooksPath .githooks

prepush:
	pnpm prepush

check: format-check shell-lint lint tsc test-coverage rust-audit rust-clippy rust-test release-check

ci: check
	pnpm build

clean:
	cd src-tauri && cargo clean
	rm -rf coverage dist node_modules/.vite

# Drops Rust build copies no recent build has used; `make dev` runs it daily.
prune:
	node scripts/prune-target.mjs

clean-dev:
	cargo clean --manifest-path src-tauri/Cargo.toml --profile dev
