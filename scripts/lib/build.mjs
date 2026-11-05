// What the scripts that build a program do alike.
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

export const root = resolve(import.meta.dirname, "..", "..");

/** `fail` and `run`, whose failures name what is being built. */
export function building(what) {
  function fail(message) {
    console.error(`${what} build failed: ${message}`);
    process.exit(1);
  }

  function run(command, args, options = {}) {
    const result = spawnSync(command, args, {
      cwd: options.cwd ?? root,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      env: { ...process.env, ...options.env },
      stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
    });
    if (result.error) fail(`${command}: ${result.error.message}`);
    if (result.status !== 0)
      fail(`${command} exited with status ${result.status}`);
    return result.stdout?.trim() ?? "";
  }

  function hostTriple() {
    const details = run("rustc", ["-vV"], { capture: true });
    const host = details.match(/^host:\s*(\S+)$/m)?.[1];
    if (!host) fail("could not determine the Rust host target");
    return host;
  }

  /** Swift links its toolchain's folder as a place to look, which a person's Mac may lack. */
  function dropToolchainPaths(binary) {
    const commands = run("otool", ["-l", binary], { capture: true });
    const paths = [
      ...commands.matchAll(/^\s+path (\/Applications\/\S+) \(offset \d+\)$/gm),
    ].map((match) => match[1]);
    for (const path of new Set(paths))
      run("install_name_tool", ["-delete_rpath", path, binary]);
  }

  function checkStarts(binary, name) {
    const version = run(binary, ["--version"], { capture: true });
    if (!version.startsWith(name))
      fail(`unexpected --version output: ${version}`);
  }

  return { fail, run, hostTriple, dropToolchainPaths, checkStarts };
}
