.PHONY: dev dev-stop build run format hooks check clean clean-dev prune

# Homebrew's Rust ignores rust-toolchain.toml, so rustup's proxies must come first.
export PATH := $(HOME)/.cargo/bin:$(PATH)

dev: export BITBUCKET_OAUTH_SECRET ?= $(shell sed -n 's/^BITBUCKET_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export JIRA_OAUTH_SECRET ?= $(shell sed -n 's/^JIRA_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export VERCEL_OAUTH_SECRET ?= $(shell sed -n 's/^VERCEL_OAUTH_SECRET=//p' .env 2>/dev/null)
dev: export KLIPY_API_KEY ?= $(shell sed -n 's/^KLIPY_API_KEY=//p' .env 2>/dev/null)
dev:
	@node scripts/prune-target.mjs --throttled >/dev/null 2>&1
	node scripts/dev-gpui.mjs

# Stops the dev build's core, the terminals and agents it runs, and its notch.
dev-stop:
	@test -x target/debug/sikemux-host || { echo "No dev build yet; run make dev first."; exit 1; }
	target/debug/sikemux-host stop
	@pid=$$(cat "$(HOME)/.config/sikemux/notch.dev.lock" 2>/dev/null); [ -z "$$pid" ] || kill "$$pid" 2>/dev/null || true

# The app and its DMG, against the size limit the Release workflow would otherwise be first to hit.
build:
	./scripts/build-mac.sh

run:
	open target/release/bundle/macos/Sikemux.app

format:
	pnpm format
	pnpm rust:fmt

hooks:
	pnpm prepare

check:
	pnpm check

clean:
	cargo clean

# Drops Rust build copies no recent build has used; `make dev` runs it hourly.
prune:
	node scripts/prune-target.mjs

clean-dev:
	cargo clean --profile dev
