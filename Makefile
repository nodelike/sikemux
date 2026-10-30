.PHONY: dev dev-stop build preflight run icons format format-check lint tsc rust-fmt rust-clippy rust-test rust-audit shell-lint release-check hooks prepush check ci clean clean-dev prune

# Homebrew's Rust ignores rust-toolchain.toml, so rustup's proxies must come first.
export PATH := $(HOME)/.cargo/bin:$(PATH)

icons:
	./scripts/icons.sh

dev: export BITBUCKET_OAUTH_SECRET ?= $(shell sed -n 's/^BITBUCKET_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export JIRA_OAUTH_SECRET ?= $(shell sed -n 's/^JIRA_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export VERCEL_OAUTH_SECRET ?= $(shell sed -n 's/^VERCEL_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export KLIPY_API_KEY ?= $(shell sed -n 's/^KLIPY_API_KEY=//p' .env 2>/dev/null)
dev: icons
	@node scripts/prune-target.mjs --throttled >/dev/null 2>&1
	node scripts/dev-gpui.mjs

# Stops the dev build's core, the terminals and agents it runs, and its notch.
dev-stop:
	@test -x target/debug/sikemux-host || { echo "No dev build yet; run make dev first."; exit 1; }
	target/debug/sikemux-host stop
	@pid=$$(cat "$(HOME)/.config/sikemux/notch.dev.lock" 2>/dev/null); [ -z "$$pid" ] || kill "$$pid" 2>/dev/null || true

# Release artifacts target the host architecture (Apple Silicon on the
# supported release machine).
build: icons
	./scripts/build-mac.sh

# Before tagging a release: the checks only the Release workflow would otherwise reach.
preflight: build

run:
	open target/release/bundle/macos/Sikemux.app

format:
	pnpm format
	pnpm rust:fmt

format-check:
	pnpm format:check
	pnpm rust:fmt:check

lint:
	pnpm lint

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

check:
	pnpm check

ci: check

clean:
	cargo clean

# Drops Rust build copies no recent build has used; `make dev` runs it daily.
prune:
	node scripts/prune-target.mjs

clean-dev:
	cargo clean --profile dev
