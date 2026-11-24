#!/usr/bin/env node
// Lists what the commits after <from>, up to <to>, changed for people, one bullet a change,
// oldest first: a commit's `Docs:` lines, or its subject when it has none. Prints nothing
// when nobody would notice any of them.
import { execFileSync } from "node:child_process";

const CODE = {
  app: /^(app\/(src|crates|assets|locales)|crates|plugins|helpers|shared|packaging|tools)\/|^Cargo\.toml$/,
  phone: /^(mobile\/(app|ios|native)|crates\/sikemux-(mobile|wire|client))\//,
};
const PHONES = /^(mobile|crates\/sikemux-mobile)\//;
const PHONE_WORDS = /\b(phones?|android|iphone|ios)\b/i;
const MAC = /^app\/src\//;
const UNSEEN_FILES =
  /(^|[/_])tests?(\/|\.\w+$)|[/._-]test\.\w+$|^app\/src\/drive\.rs$|\/Cargo\.toml$|\.(md|lock)$|^mobile\/(app\/scripts|ios\/\w*Tests)\//;
const UNSEEN_KINDS =
  /^(test|chore|docs|ci|build|refactor|style|film|shots|drive|deps|release|server|deploy)$| (test|check)s?$/;
const UNSEEN_WORDS = /\b(tests?|unused|(byte|schema) budgets?|swift-format)\b/;
const READABLE = 160;

const [audience, from, to] = process.argv.slice(2);
if (!CODE[audience] || !to) {
  console.error("Usage: release-notes.mjs app|phone <from> <to>");
  process.exit(2);
}

function commits() {
  const log = execFileSync(
    "git",
    [
      "log",
      "--reverse",
      "--no-merges",
      "--no-renames",
      "--name-only",
      "--format=%x01%H%x00%s%x00%b%x00",
      `${from}..${to}`,
    ],
    { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 },
  );
  return log
    .split("\x01")
    .slice(1)
    .map((entry) => {
      const [sha, subject, body, files] = entry.split("\0");
      return { sha, subject, body, files: files.split("\n").filter(Boolean) };
    });
}

/** What each `Docs:` line says, its wrapped lines joined. */
function docsLines(body) {
  const said = [];
  let open = false;
  for (const line of body.split("\n")) {
    if (line.startsWith("Docs: ")) {
      said.push(line.slice("Docs: ".length).trim());
      open = true;
    } else if (!line.trim() || /^[A-Z][A-Za-z-]*: /.test(line)) open = false;
    else if (open) said[said.length - 1] += ` ${line.trim()}`;
  }
  return said;
}

/** `fix(rail)!: the chip waits` as its kind `fix`, area `rail` and words. */
function parts(subject) {
  const [, kind, area, words] =
    /^([^:()`"]{1,30})(?:\(([^)]*)\))?!?: (.*)$/.exec(subject) ?? [];
  return words ? { kind, area: area ?? kind, words } : { words: subject };
}

/** Its first sentence, and no more of a long one than reads in a line. */
function readable(text) {
  let line = /^.*?[^.]\.(?=\s|$)/.exec(text)?.[0] ?? text;
  if (line.length > READABLE) {
    const clause = line.lastIndexOf("; ", READABLE);
    const pause = line.lastIndexOf(", ", READABLE);
    if (clause > 0) line = line.slice(0, clause);
    else if (pause > 0) line = `${line.slice(0, pause)}…`;
  }
  // GitHub would read a bare tag as HTML, and an @name as a person to notify.
  return line
    .replace(/\.$/, "")
    .replace(/`[^`]*`|<[^<>]+>|(?<!\w)@\w+/g, (it) =>
      it.startsWith("`") ? it : `\`${it}\``,
    )
    .replace(/^ui\b/, "UI")
    .replace(/^(?!ios\b)[a-z](?![\w.-]*_)/, (first) => first.toUpperCase())
    .replace(/^ios\b/, "iOS");
}

function notes({ subject, body, files }) {
  const docs = docsLines(body);
  const said = docs.filter((line) => !/^none\b/i.test(line));
  const code = files.some(
    (file) => CODE[audience].test(file) && !UNSEEN_FILES.test(file),
  );
  if (said.length) {
    if (audience === "phone") {
      const mac = files.some((file) => MAC.test(file));
      const here = said.filter((line) => !mac || PHONE_WORDS.test(line));
      return code ? here.map(readable) : [];
    }
    return files.every((file) => PHONES.test(file)) ? [] : said.map(readable);
  }
  if (!code || docs.some((line) => /^none\.?$/i.test(line))) return [];
  const { kind, area, words } = parts(subject);
  if ([kind, area].some((it) => it && UNSEEN_KINDS.test(it))) return [];
  if (UNSEEN_WORDS.test(words)) return [];
  return [readable(area ? `${area}: ${words}` : words)];
}

const all = commits();
const undone = new Set();
for (const revert of all.filter(({ subject }) => /^revert\b/i.test(subject))) {
  const named = /\b[0-9a-f]{7,40}\b/.exec(revert.body)?.[0];
  const reverted = named && all.find(({ sha }) => sha.startsWith(named));
  if (reverted) undone.add(reverted).add(revert);
}
const listed = all.filter((commit) => !undone.has(commit)).flatMap(notes);
for (const line of new Set(listed)) console.log(`- ${line}`);
