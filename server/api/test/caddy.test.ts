import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const CADDYFILE = readFileSync(
  new URL("../../deploy/sikemux.caddy", import.meta.url),
  "utf8",
);

const MiB = 1024 * 1024;
const SEALED_BYTES = 87_400 + 256;
const WRAP_BYTES = 256 + 128;
const VOUCH_BYTES = 180_000 + 512;

interface BodyRule {
  matcher: string;
  expr: string;
  maxBytes: number;
}

function parseSize(raw: string): number {
  const match = /^(\d+)(KiB|MiB|GiB|B)$/.exec(raw);
  if (!match) throw new Error(`Unsupported Caddy size: ${raw}`);
  const value = Number(match[1]);
  const unit = match[2];
  if (unit === "B") return value;
  if (unit === "KiB") return value * 1024;
  if (unit === "MiB") return value * MiB;
  return value * 1024 * MiB;
}

function parseApiBodyRules(source: string): BodyRule[] {
  const apiStart = source.indexOf("api.sikemux.com {");
  const appStart = source.indexOf("app.sikemux.com {");
  expect(apiStart).toBeGreaterThanOrEqual(0);
  expect(appStart).toBeGreaterThan(apiStart);
  const block = source.slice(apiStart, appStart);

  const matchers = new Map<string, string>();
  for (const [, name, expr] of block.matchAll(/^\t(@\w+)\s+`([^`]+)`/gm)) {
    if (name && expr) matchers.set(name, expr);
  }

  const rules: BodyRule[] = [];
  for (const [, matcher, size] of block.matchAll(
    /^\trequest_body\s+(@\w+)\s*\{\s*max_size\s+(\S+)\s*\}/gm,
  )) {
    if (!matcher || !size) continue;
    const expr = matchers.get(matcher);
    if (!expr) throw new Error(`Missing expression matcher ${matcher}`);
    rules.push({ matcher, expr, maxBytes: parseSize(size) });
  }
  return rules;
}

function pathMatches(pattern: string, path: string): boolean {
  const escaped = pattern
    .replace(/[.+?^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*");
  return new RegExp(`^${escaped}$`).test(path);
}

/** Evaluates Caddy's `method(...)` and `path(...)` CEL expressions used in api.sikemux.com. */
function evalCel(expr: string, method: string, path: string): boolean {
  const js = expr
    .replace(
      /method\(([^)]+)\)/g,
      (_, args: string) => `[${args}].includes(${JSON.stringify(method)})`,
    )
    .replace(
      /path\(([^)]+)\)/g,
      (_, args: string) =>
        `[${args}].some((pattern) => pathMatches(pattern, ${JSON.stringify(path)}))`,
    );
  const evaluate = new Function("pathMatches", `return Boolean(${js});`) as (
    matcher: typeof pathMatches,
  ) => boolean;
  return evaluate(pathMatches);
}

describe("sikemux.caddy api.sikemux.com request_body limits", () => {
  const rules = parseApiBodyRules(CADDYFILE);

  function effectiveLimits(method: string, path: string): BodyRule[] {
    return rules.filter((rule) => evalCel(rule.expr, method, path));
  }

  it("exempts vault rotations, keys, and vouches from @small_body and covers Hono route caps", () => {
    const orgId = "00000000-0000-0000-0000-000000000001";
    const cases: Array<{ method: string; path: string; honoMaxBytes: number }> =
      [
        {
          method: "POST",
          path: "/v1/vault/rotate",
          honoMaxBytes: 552 * SEALED_BYTES,
        },
        {
          method: "POST",
          path: `/v1/orgs/${orgId}/vault/rotate`,
          honoMaxBytes: 501 * WRAP_BYTES + 501 * SEALED_BYTES + 8 * VOUCH_BYTES,
        },
        {
          method: "PUT",
          path: "/v1/vault/keys",
          honoMaxBytes: 51 * SEALED_BYTES,
        },
        {
          method: "PUT",
          path: `/v1/orgs/${orgId}/vault/keys`,
          honoMaxBytes: 501 * WRAP_BYTES + 8 * VOUCH_BYTES,
        },
        {
          method: "PUT",
          path: `/v1/orgs/${orgId}/vault/vouches`,
          honoMaxBytes: 16 * VOUCH_BYTES,
        },
        {
          method: "POST",
          path: "/v1/stash",
          honoMaxBytes: 25 * MiB,
        },
        {
          method: "PUT",
          path: "/v1/pages/abcdefghijklmnopqrstuv",
          honoMaxBytes: 25 * MiB,
        },
        {
          method: "PUT",
          path: `/v1/mods/${"a".repeat(32)}/files/${"b".repeat(64)}`,
          honoMaxBytes: 25 * MiB + 64,
        },
        {
          method: "POST",
          path: "/v1/mods/documents",
          honoMaxBytes: 4 * MiB,
        },
      ];

    for (const { method, path, honoMaxBytes } of cases) {
      const matched = effectiveLimits(method, path);
      expect(
        matched.map((r) => r.matcher),
        `${method} ${path} must match exactly one request_body rule and not @small_body`,
      ).toHaveLength(1);
      const [rule] = matched;
      expect(rule?.matcher).not.toBe("@small_body");
      expect(rule?.maxBytes).toBeGreaterThanOrEqual(honoMaxBytes);
    }
  });

  it("still caps standard small routes at 1 MiB", () => {
    const orgId = "00000000-0000-0000-0000-000000000001";
    for (const [method, path] of [
      ["POST", "/v1/vault"],
      ["PUT", `/v1/orgs/${orgId}/logo`],
      ["POST", `/v1/orgs/${orgId}/vault/remake`],
      ["POST", "/v1/devices"],
    ] as const) {
      const matched = effectiveLimits(method, path);
      expect(matched.map((r) => r.matcher)).toEqual(["@small_body"]);
      expect(matched[0]?.maxBytes).toBe(1 * MiB);
    }
  });
});
