import type {
  Channel,
  Device,
  DeviceRegistration,
  DeviceRole,
  Platform,
} from "@sikemux/protocol";
import { sql, type Kysely, type Selectable } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import { consumeChallenge } from "./challenge.ts";
import { registrationMessage, signedBy } from "./signature.ts";

export function toDevice(row: Selectable<Tables["devices"]>): Device {
  const device: Device = {
    key: row.key,
    role: row.role as DeviceRole,
    name: row.name,
    platform: row.platform as Platform,
    createdAt: row.created_at.toISOString(),
    lastSeenAt: row.last_seen_at?.toISOString() ?? null,
  };
  if (row.channel) device.channel = row.channel as Channel;
  return device;
}

/** Who is registering: the account, how it signed in, and a phone's Clerk session. */
export interface Registrant {
  userId: string;
  via: string;
  sessionId?: string | undefined;
}

/**
 * Adds a device to the account, or updates it, once its signature over the account's challenge
 * proves it holds its key. Every way a device joins an account comes through here.
 */
export async function registerDevice(
  db: Kysely<Tables>,
  { userId, via, sessionId }: Registrant,
  registration: DeviceRegistration,
): Promise<{ row: Selectable<Tables["devices"]>; added: boolean }> {
  if ((registration.role === "host") !== (registration.channel !== undefined)) {
    throw new ApiFailure(
      400,
      "bad_request",
      "Hosts say which channel they run; clients do not.",
    );
  }
  const name = registration.name.trim();
  if (!name)
    throw new ApiFailure(400, "bad_request", "The device needs a name.");

  await consumeChallenge(db, userId, registration.nonce);
  const message = registrationMessage(
    registration.nonce,
    userId,
    registration.key,
  );
  if (!signedBy(registration.key, message, registration.signature)) {
    throw new ApiFailure(
      403,
      "forbidden",
      "The signature does not prove this device holds the key.",
    );
  }

  return db.transaction().execute(async (trx) => {
    await trx
      .insertInto("users")
      .values({ id: userId })
      .onConflict((oc) => oc.column("id").doNothing())
      .execute();
    const user = await trx
      .selectFrom("users")
      .select("deleted_at")
      .where("id", "=", userId)
      .forShare()
      .executeTakeFirstOrThrow();
    if (user.deleted_at)
      throw new ApiFailure(
        401,
        "unauthorized",
        "This account was deleted. Sign in to the phone app or app.sikemux.com to restore it.",
      );
    const existing = await trx
      .selectFrom("devices")
      .selectAll()
      .where("key", "=", registration.key)
      .forUpdate()
      .executeTakeFirst();
    if (existing && existing.user_id !== userId) {
      throw new ApiFailure(
        409,
        "conflict",
        "This device is registered to another account.",
      );
    }
    if (existing && existing.role !== registration.role) {
      throw new ApiFailure(
        409,
        "conflict",
        `This device is already registered as a ${existing.role}.`,
      );
    }
    const fields = {
      name,
      platform: registration.platform,
      channel: registration.channel ?? null,
      clerk_session_id:
        registration.role === "client" ? (sessionId ?? null) : null,
    };
    const eventId = await appendEvent(trx, {
      userId,
      type: existing ? "device.changed" : "device.added",
      subject: registration.key,
      subjectRole: registration.role,
    });
    const row = existing
      ? await trx
          .updateTable("devices")
          .set({ ...fields, updated_at: sql<Date>`now()` })
          .where("key", "=", registration.key)
          .returningAll()
          .executeTakeFirstOrThrow()
      : await trx
          .insertInto("devices")
          .values({
            key: registration.key,
            user_id: userId,
            role: registration.role,
            acked_event_id: eventId,
            ...fields,
          })
          .returningAll()
          .executeTakeFirstOrThrow();
    if (!existing)
      await trx
        .deleteFrom("removed_devices")
        .where("key", "=", registration.key)
        .execute();
    await audit(trx, {
      userId,
      actor: `user:${userId}`,
      action: existing ? "device.updated" : "device.registered",
      subject: registration.key,
      detail: {
        role: row.role,
        name: row.name,
        platform: row.platform,
        via,
      },
    });
    return { row, added: !existing };
  });
}
