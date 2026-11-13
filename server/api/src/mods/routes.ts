import type { Services } from "../app.ts";
import { Hono, type Context } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { ApiFailure, badRequest, limitBody, readBody } from "../http.ts";
import { limit, perUser } from "../limits.ts";
import type { StashFiles } from "../stash/files.ts";
import { removeStashFiles } from "../stash/files.ts";
import { purgeMod, pushDocuments } from "./change.ts";
import { pullDocuments } from "./documents.ts";
import { listFiles, putFile, readFile, removeFile } from "./files.ts";

const MOD_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;
const SEQ = /^(0|[1-9][0-9]{0,15})$/;
const FILE_ID = /^[0-9a-f]{64}$/;
const PUSH_BYTES = 4 * 1024 * 1024;

function modId(c: Context<AuthEnv>): string {
  const mod = c.req.param("mod") ?? "";
  if (!MOD_ID.test(mod))
    throw badRequest(
      "A mod's id is lowercase letters, digits and dashes, not starting with a dash.",
    );
  return mod;
}

function modFile(c: Context<AuthEnv>) {
  const file = c.req.param("file") ?? "";
  if (!FILE_ID.test(file))
    throw badRequest("A mod file's id is 64 lowercase hex digits.");
  return { mod: modId(c), file };
}

function asker(c: Context<AuthEnv>) {
  const { userId, deviceKey } = c.get("identity");
  if (!deviceKey)
    throw new ApiFailure(403, "forbidden", "Only a host keeps mod documents.");
  return { userId, hostKey: deviceKey };
}

/**
 * The account's record of every mod's documents and files, sealed under the vault key. Hosts keep
 * their own copy and merge through this one; the server sees mod ids, sizes and times, never
 * contents. Files are kept in the stash's encrypted file store, apart from its items.
 */
export function modRoutes({
  database: { db },
  verifier,
  limiter,
  stash,
}: Pick<Services, "database" | "verifier" | "limiter" | "stash">) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });
  const asUploadingHost = requireIdentity(verifier, db, {
    accept: ["host"],
    streamed: () => true,
  });
  const stored = (): StashFiles => {
    if (!stash)
      throw new ApiFailure(
        503,
        "unavailable",
        "This server keeps no files for mods.",
      );
    return stash;
  };

  return new Hono<AuthEnv>()
    .post(
      "/documents",
      limitBody<AuthEnv>(PUSH_BYTES),
      asHost,
      limit<AuthEnv>(limiter, "mods-push", 30, (c) => asker(c).hostKey),
      async (c) => {
        const body = await readBody(c, "ModsPush");
        return c.json(await pushDocuments(db, asker(c), body));
      },
    )
    .get(
      "/documents",
      asHost,
      perUser(limiter, "mods-pull", 240),
      async (c) => {
        const since = c.req.query("since") ?? "";
        if (!SEQ.test(since))
          throw badRequest(
            "Send the last seq this host saw as ?since=<number>.",
          );
        return c.json(await pullDocuments(db, asker(c).userId, Number(since)));
      },
    )
    .delete(
      "/:mod/documents",
      asHost,
      perUser(limiter, "mods-purge", 30),
      async (c) => {
        const { storedAs, ...purged } = await purgeMod(db, asker(c), modId(c));
        if (stash) await removeStashFiles(stash.dir, storedAs);
        return c.json(purged);
      },
    )
    .get(
      "/:mod/files",
      asHost,
      perUser(limiter, "mods-files", 240),
      async (c) => c.json(await listFiles(db, asker(c).userId, modId(c))),
    )
    .put(
      "/:mod/files/:file",
      asUploadingHost,
      limit<AuthEnv>(limiter, "mods-file-put", 120, (c) => asker(c).hostKey),
      async (c) =>
        c.json(
          await putFile(
            db,
            stored(),
            asker(c),
            modFile(c),
            Number(c.req.header("content-length") ?? 0),
            c.req.raw.body ?? (async function* () {})(),
            c.get("checkBody"),
          ),
        ),
    )
    .get(
      "/:mod/files/:file",
      asHost,
      perUser(limiter, "mods-file-get", 600),
      async (c) => {
        const file = modFile(c);
        const stream = await readFile(
          db,
          stored(),
          asker(c).userId,
          file,
          c.get("log"),
        );
        return c.body(stream, 200, {
          "content-type": "application/octet-stream",
          "cache-control": "private, no-store",
        });
      },
    )
    .delete(
      "/:mod/files/:file",
      asHost,
      perUser(limiter, "mods-file-remove", 240),
      async (c) => {
        await removeFile(db, stash, asker(c).userId, modFile(c));
        return c.body(null, 204);
      },
    );
}
