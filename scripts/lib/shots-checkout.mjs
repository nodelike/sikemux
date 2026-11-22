import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** Set on every git call a shots.mjs that keeps throwaway cores out of the person's browser makes. */
export const SAFE_SHOOTER = { SIKEMUX_SHOTS_SAFE: "1" };

/*
 * The pictures checkout is shared by every copy of shots.mjs, including the old one on a branch
 * behind main. Those let a signed-out agy open Google's sign-in page in the person's browser, so
 * the checkout's own hook turns away any shooter that doesn't say it is safe.
 */
export function guardCheckout(checkout) {
  const hooks = join(checkout, ".git/shots-hooks");
  mkdirSync(hooks, { recursive: true });
  writeFileSync(
    join(hooks, "post-checkout"),
    `#!/bin/sh
[ "$SIKEMUX_SHOTS_SAFE" = 1 ] && exit 0
echo "this scripts/shots.mjs is older than main's and could open sign-in pages in the person's browser; rebase on main and run it again" >&2
exit 1
`,
    { mode: 0o755 },
  );
  execFileSync("git", ["config", "core.hooksPath", hooks], { cwd: checkout });
}
