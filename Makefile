.PHONY: dev dev-stop build run format autofix hooks check clean clean-dev prune

export PATH := $(CURDIR)/scripts/bin:$(PATH)

dev:
	@test -x target/debug/sikemux-host && target/debug/sikemux-host prune-target --throttled >/dev/null 2>&1 || true
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
	@if [ -x target/debug/sikemux-host ]; then target/debug/sikemux-host prune-target; else cargo run -q -p sikemux-host --bin sikemux-host -- prune-target; fi

clean-dev:
	cargo clean --profile dev
