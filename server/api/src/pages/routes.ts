import type { Context } from "hono";
import { Hono } from "hono";

import type { Services } from "../app.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import { ApiFailure, badRequest, notFound } from "../http.ts";
import { clientAddress, limit, perUser } from "../limits.ts";
import type { StashFiles } from "../stash/files.ts";
import {
  countView,
  MAX_PAGE_DAYS,
  pageState,
  putPage,
  shownPage,
  stopPage,
  type PageSettings,
} from "./store.ts";

const PAGE_ID = /^[A-Za-z0-9_-]{22}$/;
const HEX_64 = /^[0-9a-f]{64}$/;
/** How far a host's clock may run ahead when it sets the longest expiry. */
const CLOCK_SLACK_MS = 60_000;

export const PAGE_HEADERS = {
  viewerCheck: "x-page-viewer-check",
  anyone: "x-page-anyone",
  expires: "x-page-expires",
  proof: "x-page-proof",
} as const;

function pageId(c: Context): string {
  const id = c.req.param("id") ?? "";
  if (!PAGE_ID.test(id)) throw notFound("No page is shared under that link.");
  return id;
}

function readSettings(c: Context): PageSettings {
  const viewerCheck = c.req.header(PAGE_HEADERS.viewerCheck) ?? "";
  if (!HEX_64.test(viewerCheck))
    throw badRequest(
      "X-Page-Viewer-Check is the viewer proof's SHA-256, in lowercase hex.",
    );
  const anyone = c.req.header(PAGE_HEADERS.anyone);
  if (anyone !== "true" && anyone !== "false")
    throw badRequest("X-Page-Anyone is true or false.");
  const expires = Number(c.req.header(PAGE_HEADERS.expires) ?? NaN);
  const now = Date.now();
  if (
    !Number.isSafeInteger(expires) ||
    expires <= now ||
    expires > now + MAX_PAGE_DAYS * 86_400_000 + CLOCK_SLACK_MS
  )
    throw badRequest(
      `X-Page-Expires is a time in Unix milliseconds, within ${MAX_PAGE_DAYS} days from now.`,
    );
  return {
    viewerCheck,
    anyone: anyone === "true",
    expiresAt: new Date(expires),
  };
}

function matchesEtag(header: string | undefined, etag: string): boolean {
  if (!header) return false;
  return header
    .split(",")
    .map((part) => part.trim())
    .some(
      (part) =>
        part === "*" ||
        part === etag ||
        (part.startsWith("W/") && part.slice(2) === etag),
    );
}

/**
 * Pages hosts share: each one sealed with the key in its link, kept in the stash's encrypted file
 * store until the host stops it or it expires. A viewer proves it holds the link to read it.
 */
export function pageRoutes({
  database: { db },
  verifier,
  limiter,
  stash,
  log,
}: Pick<Services, "database" | "verifier" | "limiter" | "stash" | "log">) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });
  const asUploadingHost = requireIdentity(verifier, db, {
    accept: ["host"],
    streamed: () => true,
  });
  const signedIn = requireIdentity(verifier, db);
  const stored = (): StashFiles => {
    if (!stash)
      throw new ApiFailure(
        503,
        "unavailable",
        "This server keeps no shared pages.",
      );
    return stash;
  };
  const hostKey = (c: Context<AuthEnv>) => c.get("identity").deviceKey ?? "";

  return new Hono<AuthEnv>()
    .put(
      "/:id",
      asUploadingHost,
      limit<AuthEnv>(limiter, "pages-put", 60, hostKey),
      async (c) =>
        c.json(
          await putPage(
            db,
            stored(),
            { userId: c.get("identity").userId, hostKey: hostKey(c) },
            pageId(c),
            readSettings(c),
            Number(c.req.header("content-length") ?? 0),
            c.req.raw.body ?? (async function* () {})(),
            c.get("checkBody"),
            c.get("log"),
          ),
        ),
    )
    .delete("/:id", asHost, perUser(limiter, "pages-stop", 120), async (c) =>
      c.json(await stopPage(db, stash, c.get("identity").userId, pageId(c))),
    )
    .get(
      "/:id/state",
      asHost,
      perUser(limiter, "pages-state", 240),
      async (c) =>
        c.json(await pageState(db, c.get("identity").userId, pageId(c))),
    )
    .get(
      "/:id",
      limit(limiter, "pages-view", 120, clientAddress),
      async (c) => {
        const id = pageId(c);
        const proof = c.req.header(PAGE_HEADERS.proof) ?? "";
        if (!HEX_64.test(proof))
          throw notFound("No page is shared under that link.");
        const page = await shownPage(db, id, proof);
        if (!page.anyone) await signedIn(c, async () => {});
        const etag = `"${page.storedAs}"`;
        if (matchesEtag(c.req.header("if-none-match"), etag)) {
          return c.body(null, 304, {
            etag,
            "cache-control": "private, no-store",
          });
        }
        const stream = await stored().read(page.storedAs, c.get("log"));
        if (!stream) {
          log.error({ id }, "a shared page's file is missing");
          throw notFound("No page is shared under that link.");
        }
        await countView(db, id);
        return c.body(stream, 200, {
          etag,
          "content-type": "text/plain; charset=utf-8",
          "x-content-type-options": "nosniff",
          "content-security-policy": "sandbox; default-src 'none'",
          "cache-control": "private, no-store",
        });
      },
    );
}
