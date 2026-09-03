.PHONY: dev build run icons format format-check lint test test-coverage tsc rust-fmt rust-clippy rust-test release-check check ci clean clean-dev

icons:
	./scripts/icons.sh

dev: icons
	pnpm dev:desktop

# Release artifacts target the host architecture (Apple Silicon on the
# supported release machine).
build: icons
	./scripts/build-mac.sh

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

release-check:
	pnpm release:check

check: format-check lint tsc test rust-clippy rust-test release-check

ci: check
	pnpm build

clean:
	cd src-tauri && cargo clean
	rm -rf coverage dist node_modules/.vite

clean-dev:
	cargo clean --manifest-path src-tauri/Cargo.toml --profile dev
