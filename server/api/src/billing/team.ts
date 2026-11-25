import type { TeamAccess } from "@sikemux/protocol";
import type { MiddlewareHandler } from "hono";
import { sql, type ExpressionBuilder, type Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { forbidden, notFound } from "../http.ts";

/** The seats an org made in the preview has, its owner's included. */
export const PREVIEW_SEATS = 20;

/** How long a host's word that it follows nightly holds; a running host repeats it far more often. */
const FOLLOWING_HOLDS = "30 days";

/** Whether what a host in `devices` said of the releases it follows still holds. */
export const followingHolds = sql<boolean>`coalesce(followed_at > now() - interval '${sql.raw(FOLLOWING_HOLDS)}', false)`;

const CHANGE_CHANNEL =
  "Team is in preview for people who follow Sikemux's nightly releases. Choose Nightly in Settings, About, on one of your computers, then try again.";

const NOT_OPEN_NOW =
  "Team isn't open to your account now. You can still read your orgs and leave them.";

export const VAULT_SOON = "Shared keys are coming soon to the Team preview.";

/** Whether `audience` takes in more than staff. */
const openBeyondStaff = sql<boolean>`exists (select 1 from team_audience where audience <> 'staff')`;

const vaultsOpen = sql<boolean>`exists (select 1 from team_audience where vaults_open)`;

/** Whether the org in `orgs.id` has an active Team subscription. */
export function teamPaid(eb: ExpressionBuilder<Tables, "orgs">) {
  return eb.exists(
    eb
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .select(sql`1`.as("one"))
      .whereRef("billing_customers.org_id", "=", "orgs.id")
      .where("subscriptions.product", "=", "team")
      .where("subscriptions.status", "=", "active"),
  );
}

/**
 * Whether the org in `orgs.id` has Team now: an active subscription, or its free seats, which an
 * org somebody on staff made keeps while Team is open to staff alone.
 */
export function teamActive(eb: ExpressionBuilder<Tables, "orgs">) {
  return eb.or([teamPaid(eb), previewing(eb)]);
}

function previewing(eb: ExpressionBuilder<Tables, "orgs">) {
  return eb.and([
    eb("orgs.preview_seats", "is not", null),
    eb.or([
      openBeyondStaff,
      eb.exists(
        eb
          .selectFrom("users")
          .select(sql`1`.as("one"))
          .whereRef("users.id", "=", "orgs.created_by")
          .where("users.staff", "=", true),
      ),
    ]),
  ]);
}

/** The seats the org has free, 0 outside the preview. */
export async function previewSeats(db: Kysely<Tables>, orgId: string) {
  const org = await db
    .selectFrom("orgs")
    .select("preview_seats")
    .where("id", "=", orgId)
    .where(previewing)
    .executeTakeFirst();
  return org?.preview_seats ?? 0;
}

export async function previewSeatsByOrg(
  db: Kysely<Tables>,
  orgIds: readonly string[],
): Promise<Map<string, number>> {
  const map = new Map<string, number>();
  if (!orgIds.length) return map;
  const rows = await db
    .selectFrom("orgs")
    .select(["id", "preview_seats"])
    .where("id", "in", orgIds)
    .where(previewing)
    .execute();
  for (const row of rows) map.set(row.id, row.preview_seats ?? 0);
  return map;
}

/** Whether the org in `orgs.id` has its seats from the preview, not bought. */
export function teamFree(eb: ExpressionBuilder<Tables, "orgs">) {
  return eb.and([previewing(eb), eb.not(teamPaid(eb))]);
}

export async function isFree(db: Kysely<Tables>, orgId: string) {
  const org = await db
    .selectFrom("orgs")
    .select("id")
    .where("id", "=", orgId)
    .where(teamFree)
    .executeTakeFirst();
  return org !== undefined;
}

export async function freeOrgsById(
  db: Kysely<Tables>,
  orgIds: readonly string[],
): Promise<Set<string>> {
  if (!orgIds.length) return new Set();
  const rows = await db
    .selectFrom("orgs")
    .select("id")
    .where("id", "in", orgIds)
    .where(teamFree)
    .execute();
  return new Set(rows.map((row) => row.id));
}

/**
 * How far Team is open to an account, whether or not payments are live: to staff, and to whoever
 * the audience takes in. A host says which releases it follows in a request it signs, but a
 * changed build may say anything: this decides who sees a preview and protects nothing.
 */
export async function teamAccess(
  db: Kysely<Tables>,
  live: boolean,
  userId: string,
): Promise<TeamAccess> {
  if (await mayUseTeam(db, userId)) return live ? "sold" : "preview";
  return (await isInAnOrg(db, userId)) ? "kept" : "shut";
}

/** Whether Team is open to the account, sold or free. */
export async function mayUseTeam(db: Kysely<Tables>, userId: string) {
  if (await isStaff(db, userId)) return true;
  const audience = await teamAudience(db);
  if (audience !== "nightly") return audience === "everyone";
  return Boolean(
    await db
      .selectFrom("devices")
      .select("key")
      .where("user_id", "=", userId)
      .where("role", "=", "host")
      .where("followed_channel", "in", ["nightly", "staff"])
      .where(followingHolds)
      .executeTakeFirst(),
  );
}

export async function teamAudience(db: Kysely<Tables>) {
  const row = await db
    .selectFrom("team_audience")
    .select("audience")
    .executeTakeFirstOrThrow();
  return row.audience;
}

/** Whether the account in `users.id` may hold an org's key: staff, or anyone once vaults are open. */
export function mayHoldVaults(eb: ExpressionBuilder<Tables, "users">) {
  return eb.or([eb("users.staff", "=", true), vaultsOpen]);
}

/** Whether the account may take an org's key and change its vault now. */
export async function holdsVaults(db: Kysely<Tables>, userId: string) {
  return Boolean(
    await db
      .selectFrom("users")
      .select("id")
      .where("id", "=", userId)
      .where(mayHoldVaults)
      .executeTakeFirst(),
  );
}

/** What anyone in an org may do though Team is not open to them: read, hand it over, take people out and leave. */
const isKept = (method: string, path: string) =>
  method === "GET" ||
  (method === "DELETE" && !path.endsWith("/logo")) ||
  path.endsWith("/leave") ||
  path.endsWith("/role");

async function isInAnOrg(db: Kysely<Tables>, userId: string) {
  return Boolean(
    await db
      .selectFrom("org_users")
      .select("org_id")
      .where("user_id", "=", userId)
      .executeTakeFirst(),
  );
}

/**
 * After requireIdentity: lets through accounts Team is open to, and anyone already in an org to
 * what they keep. While nightly has Team the rest hear how to get in; otherwise the route does
 * not exist for them, as for any unknown path.
 */
export function requireTeam(
  db: Kysely<Tables>,
  live: boolean,
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    const { method, path } = c.req;
    const access = await teamAccess(db, live, c.get("identity").userId);
    if (access === "shut" || (access === "kept" && !isKept(method, path))) {
      if ((await teamAudience(db)) === "nightly")
        throw forbidden(CHANGE_CHANNEL);
      if (access === "kept") throw forbidden(NOT_OPEN_NOW);
      throw notFound(`No route matches ${method} ${path}.`);
    }
    await next();
  };
}
