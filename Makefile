.PHONY: dev dev-stop build run format autofix hooks check clean clean-dev prune

export PATH := $(CURDIR)/scripts/bin:$(PATH)

dev:
	@node scripts/prune-target.mjs --throttled >/dev/null 2>&1
	node scripts/dev-gpui.mjs

# Stops the dev build's core, the terminals and agents it runs, and its notch.
dev-stop:
	@test -x target/debug/sikemux-host || { echo "No dev build yet; run make dev first."; exit 1; }
	env -u SIKEMUX_CORE_SOCKET target/debug/sikemux-host stop
	@# The lock outlives the notch, and by then its pid may belong to another program.
	@pid=$$(cat "$(HOME)/.config/sikemux/notch.dev.lock" 2>/dev/null); [ -n "$$pid" ] && ps -p "$$pid" -o comm= | grep -q '/sikemux-notch$$' && kill "$$pid" 2>/dev/null || true

# The app and its DMG, against the size limit the Release workflow would otherwise be first to hit.
build:
	./scripts/build-mac.sh

run:
	open target/release/bundle/macos/Sikemux.app

format:
	pnpm format
	pnpm rust:fmt

autofix:
	scripts/autofix.sh

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
