import type {
  Channel,
  Device,
  DeviceRegistration,
  DeviceRole,
  Platform,
} from "@sikemux/protocol";
import { sql, type Kysely, type Selectable } from "kysely";

import { audit } from "../audit.ts";
import { accountDeleted } from "../auth.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, badRequest, forbidden } from "../http.ts";
import { proveKey } from "./challenge.ts";
import { registrationMessage } from "./signature.ts";

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

/**
 * Who is registering: the account, how it signed in, and a phone's Clerk session. A host signs in
 * to add the app on it as a client, which then joins the account's other hosts as a phone does.
 */
export interface Registrant {
  userId: string;
  via: string;
  sessionId?: string | undefined;
  /** The host that signed the request, when one did. */
  deviceKey?: string | undefined;
}

/**
 * Adds a device to the account, or updates it, once its signature over the account's challenge
 * proves it holds its key. Every way a device joins an account comes through here.
 */
export async function registerDevice(
  db: Kysely<Tables>,
  { userId, via, sessionId, deviceKey }: Registrant,
  registration: DeviceRegistration,
): Promise<{ row: Selectable<Tables["devices"]>; added: boolean }> {
  if (via === "host" && registration.role !== "client")
    throw forbidden("A host may add only its own app to the account.");
  if ((registration.role === "host") !== (registration.channel !== undefined)) {
    throw badRequest("Hosts say which channel they run; clients do not.");
  }
  const name = registration.name.trim();
  if (!name) throw badRequest("The device needs a name.");

  await proveKey(db, userId, {
    nonce: registration.nonce,
    key: registration.key,
    message: registrationMessage(registration.nonce, userId, registration.key),
    signature: registration.signature,
  });

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
    if (user.deleted_at) throw accountDeleted();
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
    if (
      existing &&
      via === "host" &&
      existing.registered_by !== (deviceKey ?? null)
    ) {
      throw new ApiFailure(
        409,
        "conflict",
        "This device was added to the account another way.",
      );
    }
    if (existing && existing.role !== registration.role) {
      throw new ApiFailure(
        409,
        "conflict",
        `This device is already registered as a ${existing.role}.`,
      );
    }
    if (!existing && via === "mac") {
      const removed = await trx
        .selectFrom("removed_devices")
        .select("reason")
        .where("key", "=", registration.key)
        .where("user_id", "=", userId)
        .where("role", "=", "host")
        .executeTakeFirst();
      if (removed?.reason === "removed")
        throw forbidden(
          "This host was removed from your account, so it cannot add itself back. Run `sikemux login` on it and enter the code it shows at app.sikemux.com/host or in Sikemux on your phone.",
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
            registered_by: via === "host" ? (deviceKey ?? null) : null,
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
