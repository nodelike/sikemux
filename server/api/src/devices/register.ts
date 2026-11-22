import type { Device, DeviceRegistration } from "@sikemux/protocol";
import { sql, type Kysely, type Selectable, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import { checkStanding, holdStanding, type Identity } from "../auth.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, badRequest, forbidden, unauthorized } from "../http.ts";
import { vaultChanged } from "../orgvault/change.ts";
import { proveKey } from "./challenge.ts";
import { registrationMessage } from "./signature.ts";

export function toDevice(row: Selectable<Tables["devices"]>): Device {
  const device: Device = {
    key: row.key,
    role: row.role,
    name: row.name,
    platform: row.platform,
    createdAt: row.created_at.toISOString(),
    lastSeenAt: row.last_seen_at?.toISOString() ?? null,
  };
  if (row.channel) device.channel = row.channel;
  return device;
}

/**
 * Who is registering: the account, how it signed in, and a phone's Clerk session. A host signs in
 * to add the app on it as a client, which then joins the account's other hosts as a phone does.
 */
interface Registrant {
  userId: string;
  via: Identity["via"] | "code";
  /** The Clerk session behind the registration: the caller's, or the one that approved a host's code. */
  sessionId?: string | undefined;
  /** When the Mac sign-in behind it was made: the Mac's own, or, for a code, the approving host's. */
  issuedAt?: Date | undefined;
  /** The host that signed the request, when one did. */
  deviceKey?: string | undefined;
  /** Whether the nonce is a challenge the API issued for a Mac's browser sign-in. */
  macSignIn?: boolean;
}

/**
 * Adds a device to the account, or updates it, once its signature over the account's challenge
 * proves it holds its key. Every way a device joins an account comes through here.
 */
export async function registerDevice(
  db: Kysely<Tables>,
  { userId, via, sessionId, issuedAt, deviceKey, macSignIn }: Registrant,
  registration: DeviceRegistration,
): Promise<{ row: Selectable<Tables["devices"]>; added: boolean }> {
  if (via === "host" && registration.role !== "client")
    throw forbidden("A host may add only its own app to the account.");
  if (registration.platform === "web" && registration.role !== "client")
    throw badRequest("A browser joins hosts only as a client.");
  if (registration.handsOnly && registration.role !== "client")
    throw badRequest("Only a client is registered as hands only.");
  if ((registration.role === "host") !== (registration.channel !== undefined)) {
    throw badRequest("Hosts say which channel they run; clients do not.");
  }
  const name = registration.name.trim();
  if (!name) throw badRequest("The device needs a name.");

  await proveKey(
    db,
    userId,
    {
      nonce: registration.nonce,
      key: registration.key,
      message: registrationMessage(
        registration.nonce,
        userId,
        registration.key,
      ),
      signature: registration.signature,
    },
    { macSignIn },
  );

  return db.transaction().execute(async (trx) => {
    await holdStanding(trx, userId);
    const signInSession =
      via === "host"
        ? (await signInOf(trx, userId, deviceKey)).session
        : (sessionId ?? null);
    const macSignedInAt =
      via === "mac" || via === "code" ? issuedAt : undefined;
    await checkStanding(trx, {
      userId,
      via: via === "code" ? "session" : via,
      sessionId: signInSession ?? undefined,
      issuedAt,
      macSignedInAt,
    });
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
      // Signing in again keeps the name the account has, which the person may have chosen.
      name: existing?.name ?? name,
      platform: registration.platform,
      channel: registration.channel ?? null,
      hands_only: Boolean(existing?.hands_only || registration.handsOnly),
      clerk_session_id:
        registration.role === "client" ? (sessionId ?? null) : null,
      ...(existing
        ? {}
        : {
            sign_in_session: signInSession,
            mac_signed_in_at: macSignedInAt ?? null,
          }),
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
    if (!existing) {
      await trx
        .insertInto("ended_sessions")
        .columns(["session_id", "user_id", "made_at", "clerk_retry_at"])
        .expression(
          trx
            .selectFrom("removed_devices")
            .select([
              "clerk_session_id",
              "user_id",
              sql<Date>`now()`.as("made_at"),
              sql<Date>`now()`.as("clerk_retry_at"),
            ])
            .where("key", "=", registration.key)
            .where("reason", "=", "removed")
            .where("clerk_session_id", "is not", null)
            .where("clerk_revoked_at", "is", null)
            .where("clerk_retry_at", "is not", null),
        )
        .onConflict((conflict) => conflict.column("session_id").doNothing())
        .execute();
      await trx
        .deleteFrom("removed_devices")
        .where("key", "=", registration.key)
        .execute();
    }
    if (!existing && registration.role === "host") {
      const vaults = await trx
        .selectFrom("org_users")
        .innerJoin("org_vaults", "org_vaults.org_id", "org_users.org_id")
        .select("org_users.org_id")
        .where("org_users.user_id", "=", userId)
        .execute();
      for (const { org_id } of vaults) await vaultChanged(trx, org_id);
    }
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

/**
 * The Clerk session or Mac sign-in that put the host signing as `key` on the account, which also
 * stands behind what it adds. Read under `holdStanding`, so a host the strip just removed is refused.
 */
export async function signInOf(
  trx: Transaction<Tables>,
  userId: string,
  key: string | undefined,
): Promise<{ session: string | null; macSignedInAt: Date | null }> {
  const row = key
    ? await trx
        .selectFrom("devices")
        .select(["sign_in_session", "mac_signed_in_at"])
        .where("key", "=", key)
        .where("user_id", "=", userId)
        .executeTakeFirst()
    : undefined;
  if (!row) throw unauthorized("This host is not on an account.");
  return {
    session: row.sign_in_session,
    macSignedInAt: row.mac_signed_in_at,
  };
}
