import { spawn, spawnSync } from "node:child_process";
import path from "node:path";

const script = path.join(import.meta.dirname, "linux-checks.sh");
const env = { ...process.env, GH_TOKEN: process.env.INPUT_TOKEN };

if (spawnSync(script, ["once"], { env, stdio: "inherit" }).status !== 0)
  process.exit(1);
spawn(script, ["watch"], { env, detached: true, stdio: "ignore" }).unref();
