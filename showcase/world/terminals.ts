const ESC = "\x1b[";
const c = {
  dim: (s: string) => `${ESC}2m${s}${ESC}0m`,
  bold: (s: string) => `${ESC}1m${s}${ESC}0m`,
  green: (s: string) => `${ESC}32m${s}${ESC}0m`,
  cyan: (s: string) => `${ESC}36m${s}${ESC}0m`,
  magenta: (s: string) => `${ESC}35m${s}${ESC}0m`,
  yellow: (s: string) => `${ESC}33m${s}${ESC}0m`,
  red: (s: string) => `${ESC}31m${s}${ESC}0m`,
  blue: (s: string) => `${ESC}34m${s}${ESC}0m`,
};

const title = (text: string) => `\x1b]0;${text}\x07`;

function prompt(dir: string, branch: string, status = "") {
  const flags = status ? ` ${c.red(`[${status}]`)}` : "";
  return `\r\n${c.cyan(dir)} on ${c.magenta(` ${branch}`)}${flags}\r\n${c.green("❯")} `;
}

const lines = (...rows: string[]) => rows.join("\r\n");

const DEV = lines(
  title("pnpm dev:desktop"),
  `${prompt("~/code/sikemux", "main", "!3?1")}pnpm dev:desktop`,
  "",
  `${c.dim(">")} sikemux@0.4.2-nightly.1 dev:desktop`,
  `${c.dim(">")} pnpm sidecar:dev && node scripts/dev-desktop.mjs`,
  "",
  `   ${c.green("Compiling")} sikemux-tools-mcp v0.4.2`,
  `    ${c.green("Finished")} \`dev\` profile in 3.84s`,
  "",
  `  ${c.bold(c.green("VITE"))} ${c.green("v6.4.3")}  ready in ${c.bold("412")} ms`,
  "",
  `  ${c.green("➜")}  ${c.bold("Local")}:   ${c.cyan("http://localhost:1420/")}`,
  `  ${c.green("➜")}  ${c.dim("Network: use --host to expose")}`,
  "",
  `     ${c.green("Running")} \`cargo run\``,
  `        ${c.cyan("Info")} Watching src-tauri for changes...`,
  `   ${c.green("Compiling")} sikemux v0.4.2`,
  `    ${c.green("Finished")} \`dev\` profile in 11.02s`,
  `     ${c.green("Running")} \`target/debug/sikemux\``,
  `${c.dim("9:40:07")} ${c.cyan("[vite]")} ${c.green("hmr")} ${c.dim("/src/styles/modern-shell.css")}`,
  `${c.dim("9:40:19")} ${c.cyan("[vite]")} ${c.green("hmr")} ${c.dim("/src/components/AgentRail.tsx")}`,
);

const TEST = lines(
  title("vitest"),
  `${prompt("~/code/sikemux", "main", "!3?1")}pnpm test src/components/AgentRail`,
  "",
  ` ${c.bold(c.cyan("RUN"))}  ${c.cyan("v4.1.10")} ${c.dim("~/code/sikemux")}`,
  "",
  ` ${c.green("✓")} AgentRail.test.tsx ${c.dim("(24 tests)")} ${c.yellow("311ms")}`,
  ` ${c.green("✓")} AgentRailRows.test.tsx ${c.dim("(9 tests)")} ${c.dim("88ms")}`,
  ` ${c.green("✓")} agentActivity.test.ts ${c.dim("(17 tests)")} ${c.dim("42ms")}`,
  "",
  ` ${c.dim("Test Files")}  ${c.bold(c.green("3 passed"))} ${c.dim("(3)")}`,
  `      ${c.dim("Tests")}  ${c.bold(c.green("50 passed"))} ${c.dim("(50)")}`,
  `   ${c.dim("Start at")}  09:40:22`,
  `   ${c.dim("Duration")}  1.94s`,
  prompt("~/code/sikemux", "main", "!3?1"),
);

const GIT = lines(
  title("git"),
  `${prompt("~/code/sikemux", "main", "!3?1")}git log --oneline -6`,
  `${c.yellow("8992d13")} ${c.cyan("(HEAD -> main)")} ci: owner reviews release tooling`,
  `${c.yellow("c2c2aeb")} fix(chat): tool call URLs open as links`,
  `${c.yellow("57b46d2")} fix(chat): subagent rows hold still`,
  `${c.yellow("246d635")} fix(stage): no drift on the left edge`,
  `${c.yellow("7866fc0")} ${c.red("(tag: v0.4.1)")} feat(release): What's new credits`,
  `${c.yellow("f4c21b8")} perf(pty): replay as one buffer`,
  `${prompt("~/code/sikemux", "main", "!3?1")}git status -sb`,
  `${c.green("## main")}...${c.red("origin/main")} [ahead ${c.green("2")}]`,
  ` ${c.red("M")} src/components/AgentRail.tsx`,
  `${c.green("M")}  src/styles/modern-shell.css`,
  ` ${c.red("M")} src/styles/tokens.css`,
  `${c.red("??")} src/components/AgentRailDensity.tsx`,
  prompt("~/code/sikemux", "main", "!3?1"),
);

const FRONT_DEV = lines(
  title("astro dev"),
  `${prompt("~/code/sikemux-front", "main")}pnpm dev`,
  "",
  ` ${c.bold(c.magenta("astro"))}  ${c.green("v7.3.5")} ${c.dim("ready in")} 188 ms`,
  "",
  `┃ ${c.bold("Local")}    ${c.cyan("http://localhost:4321/")}`,
  `┃ ${c.dim("Network  use --host to expose")}`,
  "",
  `${c.dim("15:40:02")} ${c.dim("[watch]")} src/components/Hero.astro`,
  `${c.dim("15:40:02")} ${c.cyan("[200]")} / ${c.dim("31ms")}`,
  `${c.dim("15:40:48")} ${c.dim("[watch]")} src/components/DownloadButton.astro`,
  `${c.dim("15:40:48")} ${c.cyan("[200]")} / ${c.dim("24ms")}`,
);

const BENCH = lines(
  title("bench"),
  `${prompt("~/code/moodboard-studio", "palette-kmeans", "!1")}python -m benchmarks.palette --images 240`,
  "",
  `${c.bold("palette extraction")} · 240 images · 8 colours each`,
  "",
  `  ${c.dim("method".padEnd(18))}${c.dim("p50".padStart(9))}${c.dim("p95".padStart(9))}${c.dim("ΔE vs ref".padStart(12))}`,
  `  ${"median-cut".padEnd(18)}${"41 ms".padStart(9)}${"77 ms".padStart(9)}${"6.8".padStart(12)}`,
  `  ${"k-means (srgb)".padEnd(18)}${"63 ms".padStart(9)}${"118 ms".padStart(9)}${"4.1".padStart(12)}`,
  `  ${c.green("k-means (oklab)".padEnd(18))}${c.green("66 ms".padStart(9))}${c.green("121 ms".padStart(9))}${c.green("2.3".padStart(12))}`,
  "",
  `${c.green("✓")} oklab clustering is ${c.bold("44% closer")} to the reference palettes`,
  prompt("~/code/moodboard-studio", "palette-kmeans", "!1"),
);

const HERMES = lines(
  title("hermes"),
  c.dim("╭─ hermes · moodboard-studio ───────────────────────╮"),
  `${c.dim("│")} ${c.bold("❯")} write up the palette results for the PR             ${c.dim("│")}`,
  c.dim("╰────────────────────────────────────────────────────╯"),
  "",
  ` ${c.green("●")} Read ${c.cyan("benchmarks/palette.py")}`,
  ` ${c.green("●")} Ran ${c.cyan("python -m benchmarks.palette --images 240")}`,
  `   ${c.dim("k-means (oklab)  66 ms  121 ms  ΔE 2.3")}`,
  ` ${c.green("●")} Wrote ${c.cyan("docs/palette.md")} ${c.green("+38")} ${c.red("-4")}`,
  ` ${c.yellow("◐")} Opening a pull request ${c.dim("gh pr create --fill")}`,
  "",
  ` ${c.dim("hermes-4 · 12.4k tokens · esc to interrupt")}`,
);

const SHELL = lines(
  title("zsh"),
  `${prompt("~/code/sikemux", "main")}gh run list --limit 4`,
  `${c.dim("STATUS  TITLE                                   WORKFLOW  AGE")}`,
  `${c.green("✓")}       ci: owner reviews release tooling       CI        ${c.dim("12m")}`,
  `${c.green("✓")}       fix(chat): tool call URLs open as links CI        ${c.dim("1h")}`,
  `${c.yellow("*")}       v0.4.2-nightly.1                        Release   ${c.dim("2h")}`,
  `${c.green("✓")}       fix(stage): no drift on the left edge   CI        ${c.dim("3h")}`,
  prompt("~/code/sikemux", "main"),
);

const GPU = lines(
  title("gpu-box"),
  `${c.green("edon@gpu-box")}:${c.blue("~")}$ nvidia-smi --query-gpu=name,utilization.gpu,memory.used --format=csv`,
  "name, utilization.gpu [%], memory.used [MiB]",
  "NVIDIA RTX 4090, 87 %, 18342 MiB",
  `${c.green("edon@gpu-box")}:${c.blue("~")}$ `,
);

export const TERMINAL_REPLAY: Record<string, string> = {
  "t-gpu": GPU,
  "agent-notes": HERMES,
  "t-shell": SHELL,
  "t-dev": DEV,
  "t-test": TEST,
  "t-git": GIT,
  "t-front": FRONT_DEV,
  "t-mood": BENCH,
};

export function terminalReplay(
  paneId: string | undefined,
  cwd: string | null,
): string {
  if (paneId && TERMINAL_REPLAY[paneId]) return TERMINAL_REPLAY[paneId];
  const dir = cwd ? `~${cwd.replace(/^\/Users\/[^/]+/, "")}` : "~";
  return prompt(dir, "main");
}
