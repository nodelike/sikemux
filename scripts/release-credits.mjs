#!/usr/bin/env node
// Prints the credits a release ships in latest.json, so What's new needs nothing from GitHub.
// Mirrors src-tauri/src/release_credits.rs. Prints nothing for a first release.
import { execFileSync } from "node:child_process";

const REPO_API = "https://api.github.com/repos/nodelike/sikemux";
const REPO_WEB = "https://github.com/nodelike/sikemux";
const AVATAR_ORIGIN = "https://avatars.githubusercontent.com/";
const COMMITS_PER_PAGE = 100;
const MAX_COMMIT_PAGES = 20;
// The modal shows 16 faces before "+N"; the rest load when someone asks.
const BUNDLED_AVATARS = 16;
const AVATAR_PIXELS = 64;
const MAX_AVATAR_BYTES = 64 * 1024;

function fail(message) {
  console.error(`release-credits: ${message}`);
  process.exit(1);
}

const [version, commit] = process.argv.slice(2);
if (!version || !commit) fail("usage: release-credits.mjs <version> <commit>");

function parse(text) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(
    text,
  );
  if (!match) return null;
  return {
    core: match.slice(1, 4).map(Number),
    pre: match[4] ? match[4].split(".") : [],
  };
}

function compareIdentifiers(a, b) {
  const numeric = /^\d+$/;
  if (numeric.test(a) && numeric.test(b)) return Number(a) - Number(b);
  if (numeric.test(a)) return -1;
  if (numeric.test(b)) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

function compare(a, b) {
  for (let i = 0; i < 3; i++)
    if (a.core[i] !== b.core[i]) return a.core[i] - b.core[i];
  if (!a.pre.length || !b.pre.length) return b.pre.length - a.pre.length;
  for (let i = 0; i < Math.min(a.pre.length, b.pre.length); i++) {
    const order = compareIdentifiers(a.pre[i], b.pre[i]);
    if (order) return order;
  }
  return a.pre.length - b.pre.length;
}

/** A nightly follows whatever shipped last; a stable release follows the last stable one. */
function previousTag(current) {
  const tags = execFileSync("git", ["tag", "--list", "v*"], {
    encoding: "utf8",
  }).split("\n");
  let best = null;
  for (const tag of tags) {
    const candidate = parse(tag.slice(1));
    if (!candidate || (!current.pre.length && candidate.pre.length)) continue;
    if (compare(candidate, current) >= 0) continue;
    if (!best || compare(candidate, best.version) > 0)
      best = { tag, version: candidate };
  }
  return best?.tag ?? null;
}

async function getJson(url) {
  const headers = {
    accept: "application/vnd.github+json",
    "user-agent": "sikemux-release",
  };
  if (process.env.GH_TOKEN)
    headers.authorization = `Bearer ${process.env.GH_TOKEN}`;
  const response = await fetch(url, {
    headers,
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) fail(`${url} answered ${response.status}`);
  return response.json();
}

function tally(commits) {
  const people = new Map();
  for (const entry of commits) {
    const account = entry.author;
    if (!account || account.type !== "User") continue;
    const known = people.get(account.login);
    if (known) {
      known.commits += 1;
      continue;
    }
    const name = entry.commit?.author?.name?.trim();
    people.set(account.login, {
      login: account.login,
      name: name || account.login,
      commits: 1,
      avatar: account.avatar_url,
    });
  }
  return [...people.values()].sort((a, b) => b.commits - a.commits);
}

function imageType(bytes) {
  if (bytes.subarray(0, 8).equals(Buffer.from("\x89PNG\r\n\x1a\n", "latin1")))
    return "image/png";
  if (bytes.subarray(0, 3).equals(Buffer.from([0xff, 0xd8, 0xff])))
    return "image/jpeg";
  if (bytes.subarray(0, 4).toString("latin1") === "GIF8") return "image/gif";
  if (
    bytes.subarray(0, 4).toString("latin1") === "RIFF" &&
    bytes.subarray(8, 12).toString("latin1") === "WEBP"
  )
    return "image/webp";
  return null;
}

async function avatar(url) {
  if (!url.startsWith(AVATAR_ORIGIN)) return null;
  const sized = `${url}${url.includes("?") ? "&" : "?"}s=${AVATAR_PIXELS}`;
  try {
    const response = await fetch(sized, {
      signal: AbortSignal.timeout(15_000),
    });
    if (!response.ok) return null;
    const bytes = Buffer.from(await response.arrayBuffer());
    const type = bytes.length <= MAX_AVATAR_BYTES ? imageType(bytes) : null;
    return type && `data:${type};base64,${bytes.toString("base64")}`;
  } catch {
    return null;
  }
}

const current = parse(version) ?? fail(`${version} is not a semantic version`);
const previous = previousTag(current);
if (!previous) process.exit(0);

const commits = [];
let total = 0;
for (let page = 1; page <= MAX_COMMIT_PAGES; page++) {
  const comparison = await getJson(
    `${REPO_API}/compare/${previous}...${commit}?per_page=${COMMITS_PER_PAGE}&page=${page}`,
  );
  total = comparison.total_commits;
  commits.push(...comparison.commits);
  if (comparison.commits.length < COMMITS_PER_PAGE || commits.length >= total)
    break;
}
const contributors = tally(commits);
const avatars = {};
await Promise.all(
  contributors.slice(0, BUNDLED_AVATARS).map(async (person) => {
    const data = await avatar(person.avatar);
    if (data) avatars[person.avatar] = data;
  }),
);
process.stdout.write(
  JSON.stringify({
    commits: total,
    compare: `${REPO_WEB}/compare/${previous}...v${version}`,
    contributors,
    avatars,
  }),
);
