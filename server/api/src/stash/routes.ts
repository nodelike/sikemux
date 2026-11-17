import { STASH_HEADERS } from "@sikemux/protocol/stash";
import type { Services } from "../app.ts";
import type { StashTarget } from "@sikemux/protocol";
import { validator, stashTakenValues } from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import {
  ApiFailure,
  badRequest,
  limitBody,
  notFound,
  oneOf,
  readBody,
} from "../http.ts";
import { perUser } from "../limits.ts";
import { addFile, addItem, type Upload } from "./add.ts";
import { ownedItem, removeItem, shownItem, updateItem } from "./change.ts";
import type { StashFiles } from "./files.ts";
import { removeStashFiles } from "./files.ts";
import { ITEM_ID, toItem } from "./items.ts";
import { listStash, seeStash } from "./list.ts";

const NAME_MAX = 255;
const NOTE_MAX = 2000;
const MIME = /^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/;
const CONTROL = /\p{Cc}/u;

const protocol = validator();

function isUpload(c: Context): boolean {
  return (
    c.req.method === "POST" &&
    !/^application\/json\b/i.test(c.req.header("content-type") ?? "")
  );
}

function itemId(c: Context): string {
  const id = c.req.param("id") ?? "";
  if (!ITEM_ID.test(id)) throw notFound("No stashed item has that id.");
  return id;
}

function header(c: Context, name: string): string | undefined {
  const value = c.req.header(name);
  if (value === undefined) return undefined;
  try {
    return decodeURIComponent(value);
  } catch {
    throw badRequest(`${name} is not URI-encoded.`);
  }
}

function readTarget(c: Context): StashTarget | undefined {
  const text = header(c, STASH_HEADERS.target);
  if (text === undefined) return undefined;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw badRequest("X-Stash-Target is not JSON.");
  }
  const result = protocol.validate("StashTarget", value);
  if (!result.ok)
    throw badRequest(
      `X-Stash-Target is not a valid StashTarget: ${result.problems.join("; ")}.`,
    );
  return result.value;
}

/** The kind, name, type and note of an upload, from its headers. */
function readUpload(c: Context): Upload {
  const mime = (c.req.header("content-type") ?? "")
    .split(";")[0]
    ?.trim()
    .toLowerCase();
  if (!mime || mime.length > NAME_MAX || !MIME.test(mime))
    throw badRequest("A file is sent with its media type in Content-Type.");
  const name = header(c, STASH_HEADERS.name)?.trim();
  if (!name || name.length > NAME_MAX || CONTROL.test(name))
    throw badRequest(
      `A file is sent with its name in X-Stash-Name, at most ${NAME_MAX} characters.`,
    );
  const note = header(c, STASH_HEADERS.note);
  if (note !== undefined && note.length > NOTE_MAX)
    throw badRequest(`X-Stash-Note is limited to ${NOTE_MAX} characters.`);
  const kind = mime.startsWith("image/")
    ? ("image" as const)
    : mime.startsWith("video/")
      ? ("video" as const)
      : ("file" as const);
  const prompt = c.req.header(STASH_HEADERS.prompt);
  if (prompt !== undefined && !ITEM_ID.test(prompt))
    throw badRequest("X-Stash-Prompt is not a stashed item's id.");
  const target = readTarget(c);
  if (prompt !== undefined && target)
    throw badRequest(
      "An attachment goes where its prompt goes; send no target.",
    );
  const declared = Number(c.req.header("content-length") ?? 0);
  return { kind, mime, name, note: note || null, target, prompt, declared };
}

function contentDisposition(name: string): string {
  const plain = name.replace(/[^\x20-\x7e]|["\\]/g, "_");
  return `attachment; filename="${plain}"; filename*=UTF-8''${encodeURIComponent(name)}`;
}

/** The account's stash: links, text and files people share into Sikemux from other apps. */
export function stashRoutes({
  database: { db },
  verifier,
  limiter,
  stash: files,
  log,
}: Pick<Services, "database" | "verifier" | "limiter" | "stash" | "log">) {
  const jsonBodies = limitBody<AuthEnv>();

  const stored = (): StashFiles => {
    if (!files)
      throw new ApiFailure(
        503,
        "unavailable",
        "The stash is not set up on this server.",
      );
    return files;
  };

  const add = async (c: Context<AuthEnv>) =>
    isUpload(c)
      ? addFile(
          db,
          stored(),
          c.get("identity"),
          readUpload(c),
          c.req.raw.body ?? (async function* () {})(),
          c.get("checkBody"),
        )
      : addItem(db, log, c.get("identity"), await readBody(c, "StashAdd"));

  return new Hono<AuthEnv>()
    .use((c, next) => (isUpload(c) ? next() : jsonBodies(c, next)))
    .use(
      requireIdentity(verifier, db, {
        accept: ["session", "host", "device"],
        streamed: isUpload,
      }),
    )
    .post("/", perUser(limiter, "stash-add", 60), async (c) => {
      const item = toItem(await add(c));
      c.get("log").info({ id: item.id, kind: item.kind }, "stashed an item");
      return c.json(item, 201);
    })
    .get("/", perUser(limiter, "stash-list", 120), async (c) => {
      const taken = oneOf(
        "taken",
        c.req.query("taken") ?? "false",
        stashTakenValues,
      );
      return c.json(
        await listStash(
          db,
          c.get("identity").userId,
          taken,
          c.req.query("cursor"),
        ),
      );
    })
    .put("/seen", perUser(limiter, "stash-seen", 120), async (c) => {
      const { at } = await readBody(c, "StashSeen");
      return c.json(await seeStash(db, c.get("identity").userId, at));
    })
    .get("/:id", perUser(limiter, "stash-get", 240), async (c) =>
      c.json(await shownItem(db, c.get("identity").userId, itemId(c))),
    )
    .get("/:id/file", perUser(limiter, "stash-file", 120), async (c) => {
      const row = await ownedItem(db, c.get("identity").userId, itemId(c));
      if (row.size === null || row.name === null || row.mime === null)
        throw notFound("That item has no file.");
      const stream = await stored().read(row.id, c.get("log"));
      if (!stream) {
        c.get("log").error({ id: row.id }, "a stashed item's file is missing");
        throw notFound("That item's file is gone.");
      }
      return c.body(stream, 200, {
        "content-type": row.mime,
        "content-length": String(row.size),
        "content-disposition": contentDisposition(row.name),
        "content-security-policy": "sandbox; default-src 'none'",
        "cache-control": "private, no-store",
      });
    })
    .patch("/:id", perUser(limiter, "stash-update", 120), async (c) => {
      const update = await readBody(c, "StashUpdate");
      return c.json(
        await updateItem(db, c.get("identity").userId, itemId(c), update),
      );
    })
    .delete("/:id", perUser(limiter, "stash-delete", 120), async (c) => {
      const id = itemId(c);
      const withFiles = await removeItem(db, c.get("identity").userId, id);
      if (withFiles.length && files)
        await removeStashFiles(files.dir, withFiles).catch((error: unknown) =>
          c
            .get("log")
            .warn({ err: error, id }, "could not delete a stashed file"),
        );
      return c.body(null, 204);
    });
}
