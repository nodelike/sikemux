import type {
  AccountEventType,
  StashItem,
  StashTarget,
} from "@sikemux/protocol";
import { MAX_STASH_ACCOUNT_BYTES } from "@sikemux/protocol/stash";
import type { Kysely, Selectable, Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, badRequest } from "../http.ts";

export const ITEM_ID = /^[A-Za-z0-9_-]{22}$/;
export const TAKEN_KEPT_DAYS = 30;
export const UNFINISHED_PROMPT_DAYS = 1;

export type ItemRow = Selectable<Tables["stash_items"]>;

export function toItem(
  row: ItemRow,
  attachments: readonly ItemRow[] = [],
): StashItem {
  const item: StashItem = {
    id: row.id,
    kind: row.kind,
    preview: row.preview ?? null,
    target: row.target ?? null,
    createdAt: row.created_at.toISOString(),
    takenAt: row.taken_at?.toISOString() ?? null,
    fromDevice: row.from_device,
  };
  if (row.url !== null) item.url = row.url;
  if (row.text !== null) item.text = row.text;
  if (row.links !== null) item.links = row.links;
  if (row.note !== null) item.note = row.note;
  if (row.name !== null) item.name = row.name;
  if (row.mime !== null) item.mime = row.mime;
  if (row.size !== null) item.size = Number(row.size);
  if (row.kind === "prompt")
    item.attachments = attachments.map((part) => toItem(part));
  return item;
}

/** The items as clients see them, each prompt with its attachments. */
export async function withAttachments(
  db: Kysely<Tables>,
  rows: readonly ItemRow[],
): Promise<StashItem[]> {
  const prompts = rows
    .filter((row) => row.kind === "prompt")
    .map((row) => row.id);
  const parts = prompts.length
    ? await db
        .selectFrom("stash_items")
        .selectAll()
        .where("prompt_id", "in", prompts)
        .orderBy("created_at")
        .orderBy("id")
        .execute()
    : [];
  return rows.map((row) =>
    toItem(
      row,
      parts.filter((part) => part.prompt_id === row.id),
    ),
  );
}

/** The item as clients see it, a prompt with its attachments. */
export async function shownAs(
  db: Kysely<Tables>,
  row: ItemRow,
): Promise<StashItem> {
  const [item = toItem(row)] = await withAttachments(db, [row]);
  return item;
}

/** Refuses a target that is not a chat, or a new chat in a project, on one of the account's hosts. */
export async function checkTarget(
  db: Kysely<Tables>,
  userId: string,
  target: StashTarget,
): Promise<void> {
  const chat = target.chatId !== undefined;
  const newChat = target.projectPath !== undefined && target.newChat === true;
  if (chat === newChat)
    throw badRequest(
      "A target is a chatId, or a projectPath with newChat true.",
    );
  const host = await db
    .selectFrom("devices")
    .select("key")
    .where("key", "=", target.hostKey)
    .where("user_id", "=", userId)
    .where("role", "=", "host")
    .executeTakeFirst();
  if (!host) throw badRequest("The target's host is not on your account.");
}

/** Tells the account's hosts, phones and the web app that an item changed. */
export async function announce(
  trx: Transaction<Tables>,
  userId: string,
  type: Extract<AccountEventType, `stash.${string}`>,
  id: string,
): Promise<void> {
  await appendEvent(trx, { userId, type, subject: id });
}

export async function checkSpace(
  db: Kysely<Tables>,
  userId: string,
  bytes: number,
): Promise<void> {
  const { used } = await db
    .selectFrom("stash_items")
    .select((eb) =>
      eb.fn.coalesce(eb.fn.sum<string>("size"), eb.lit(0)).as("used"),
    )
    .where("user_id", "=", userId)
    .executeTakeFirstOrThrow();
  if (Number(used) + bytes > MAX_STASH_ACCOUNT_BYTES)
    throw new ApiFailure(
      413,
      "payload_too_large",
      "The stash is full: an account keeps at most 1 GB. Delete items to make room.",
    );
}
