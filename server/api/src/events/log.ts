import type {
  AccountEventType,
  DeviceRole,
  RevokeReason,
} from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import type { Tables } from "../db.ts";

/** The channel every account change is announced on, carrying only the account's id. */
export const EVENTS_CHANNEL = "sikemux_events";

interface NewEvent {
  userId: string;
  type: AccountEventType;
  subject?: string;
  subjectRole?: DeviceRole;
  reason?: RevokeReason;
}

function rowOf(event: NewEvent) {
  return {
    user_id: event.userId,
    type: event.type,
    subject: event.subject ?? null,
    subject_role: event.subjectRole ?? null,
    reason: event.reason ?? null,
  };
}

/**
 * With appendEvents, the only way an event is written. It goes in with the change it
 * describes, and the announcement leaves only if that transaction commits.
 */
export async function appendEvent(
  trx: Transaction<Tables>,
  event: NewEvent,
): Promise<number> {
  const row = await trx
    .insertInto("events")
    .values(rowOf(event))
    .returning("id")
    .executeTakeFirstOrThrow();
  await sql`select pg_notify(${EVENTS_CHANNEL}, ${event.userId})`.execute(trx);
  return Number(row.id);
}

/** Writes many events at once, announcing each account they are on once. */
export async function appendEvents(
  trx: Transaction<Tables>,
  events: readonly NewEvent[],
): Promise<void> {
  if (events.length === 0) return;
  await trx.insertInto("events").values(events.map(rowOf)).execute();
  for (const userId of new Set(events.map((event) => event.userId)))
    await sql`select pg_notify(${EVENTS_CHANNEL}, ${userId})`.execute(trx);
}

/** The account's newest event id, or 0 when it has none. */
export async function latestEventId(
  db: Kysely<Tables>,
  userId: string,
): Promise<number> {
  const row = await db
    .selectFrom("events")
    .select((eb) => eb.fn.max("id").as("latest"))
    .where("user_id", "=", userId)
    .executeTakeFirst();
  return Number(row?.latest ?? 0);
}
