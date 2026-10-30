import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const policy = JSON.parse(
  await readFile(resolve(root, "dependency-audit.json"), "utf8"),
);
const reviewBy = new Date(`${policy.reviewBy}T00:00:00Z`);
if (!Number.isFinite(reviewBy.valueOf()) || reviewBy < new Date()) {
  throw new Error(`dependency warning policy expired on ${policy.reviewBy}`);
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  return result;
}

const lockfile = "src-tauri/Cargo.lock";
const observedWarnings = new Set();
const rust = run("cargo", ["audit", "--file", lockfile, "--json"]);
let rustReport;
try {
  rustReport = JSON.parse(rust.stdout);
} catch {
  process.stderr.write(rust.stderr);
  throw new Error(`cargo audit did not return a JSON report for ${lockfile}`);
}
if (rustReport.vulnerabilities?.found) {
  throw new Error(
    `cargo audit found ${rustReport.vulnerabilities.count} vulnerable package(s) in ${lockfile}`,
  );
}
for (const [kind, entries] of Object.entries(rustReport.warnings ?? {})) {
  for (const entry of entries) {
    const id = entry.advisory?.id;
    const packageName = entry.package?.name;
    const packageVersion = entry.package?.version;
    if (id) observedWarnings.add(id);
    else if (packageName && packageVersion)
      observedWarnings.add(`cargo:${kind}:${packageName}@${packageVersion}`);
    else
      throw new Error(
        `cargo audit returned an unidentifiable ${kind} warning for ${lockfile}`,
      );
  }
}
const allowedWarnings = new Set(Object.keys(policy.allowedRustWarnings ?? {}));
const unexpected = [...observedWarnings].filter(
  (id) => !allowedWarnings.has(id),
);
const stale = [...allowedWarnings].filter((id) => !observedWarnings.has(id));
if (unexpected.length > 0)
  throw new Error(
    `unreviewed Rust dependency warnings: ${unexpected.join(", ")}`,
  );
if (stale.length > 0)
  throw new Error(
    `remove resolved Rust warning exceptions: ${stale.join(", ")}`,
  );

const node = run("pnpm", ["audit", "--prod", "--json"]);
let nodeReport;
try {
  nodeReport = JSON.parse(node.stdout);
} catch {
  process.stderr.write(node.stderr);
  throw new Error("pnpm audit did not return a JSON report");
}
const nodeVulnerabilities = nodeReport.metadata?.vulnerabilities;
const nodeTotal =
  nodeVulnerabilities?.total ?? nodeReport.advisories?.length ?? 0;
if (node.status !== 0 || nodeTotal !== 0)
  throw new Error(
    `pnpm audit found ${nodeTotal} production vulnerability/vulnerabilities`,
  );

console.log(
  `dependency health ok: no known vulnerabilities; ${observedWarnings.size} reviewed Rust warnings expire ${policy.reviewBy}`,
);
