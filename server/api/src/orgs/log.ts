import type {
  OrgAuditAction,
  OrgAuditEntry,
  OrgAuditPage,
  OrgDomainJoining,
  OrgRole,
} from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Selectable } from "kysely";

import type { Person } from "../account/clerk.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { badRequest } from "../http.ts";
import { membership, orgIdOf, requireRole } from "./shared.ts";

const PAGE = 50;

/** The audit actions the org's log shows, by the name the log gives each. */
const SHOWN: Record<string, OrgAuditAction> = {
  "org.created": "created",
  "org.joined": "joined",
  "org.left": "left",
  "org.member_removed": "removed",
  "org.role_changed": "role_changed",
  "org.invite_created": "invite_created",
  "org.invite_revoked": "invite_revoked",
  "org.team_created": "team_created",
  "org.team_renamed": "team_renamed",
  "org.team_deleted": "team_deleted",
  "org.team_member_added": "team_member_added",
  "org.team_member_removed": "team_member_removed",
  "org.domain_added": "domain_added",
  "org.domain_verified": "domain_verified",
  "org.domain_joining_changed": "domain_joining_changed",
  "org.domain_removed": "domain_removed",
};

/** What the Team plan's own changes are called in the org's log. */
const PLAN: Record<string, OrgAuditAction> = {
  "billing.subscription_changed": "seats_changed",
  "billing.subscription_canceled": "plan_canceled",
  "billing.subscription_resumed": "plan_resumed",
};

const ROLES = new Set<string>(["owner", "admin", "user"]);
const role = (value: unknown) =>
  typeof value === "string" && ROLES.has(value)
    ? (value as OrgRole)
    : undefined;
const text = (value: unknown) =>
  typeof value === "string" && value ? value : undefined;
const joining = (value: unknown) =>
  value === "open" || value === "offered"
    ? (value as OrgDomainJoining)
    : undefined;

const userOf = (actor: string) =>
  actor.startsWith("user:") ? actor.slice(5) : null;

function entryBody(
  row: Selectable<Tables["audit"]>,
  named: Map<string, Person>,
): OrgAuditEntry {
  const detail = row.detail as Record<string, unknown>;
  const by = userOf(row.actor);
  const about = row.user_id !== by ? row.user_id : null;
  const action = SHOWN[row.action] ?? PLAN[row.action];
  if (!action) throw new Error(`The org's log read a ${row.action} row`);
  const fields = {
    role: role(detail.role),
    from: role(detail.from),
    to: role(detail.to),
    team: text(detail.team),
    previousName: text(detail.previousName),
    domain: text(detail.domain),
    joining: joining(detail.joining),
    via:
      action === "joined"
        ? detail.via === "domain"
          ? ("domain" as const)
          : ("invite" as const)
        : undefined,
    seats:
      action === "seats_changed" && typeof detail.quantity === "number"
        ? detail.quantity
        : undefined,
    atRenewal:
      action === "seats_changed" ? detail.when === "renewal" : undefined,
  };
  return {
    id: String(row.id),
    at: row.at.toISOString(),
    action,
    by: by ? (named.get(by)?.name ?? null) : null,
    about: about ? (named.get(about)?.name ?? null) : null,
    ...(Object.fromEntries(
      Object.entries(fields).filter(([, value]) => value !== undefined),
    ) as Partial<OrgAuditEntry>),
  };
}

/** The org's read of the audit log: what happened to it, naming people only by their sign-in's name. */
export function auditRoutes(
  db: Kysely<Tables>,
  people: (c: Context<AuthEnv>, ids: string[]) => Promise<Map<string, Person>>,
) {
  return new Hono<AuthEnv>().get("/:orgId/audit", async (c) => {
    const orgId = orgIdOf(c);
    const { role: caller } = await membership(
      db,
      orgId,
      c.get("identity").userId,
    );
    requireRole(caller, ["owner", "admin"]);
    const cursor = c.req.query("cursor");
    if (cursor !== undefined && !/^[0-9]{1,19}$/.test(cursor))
      throw badRequest("That cursor is not one this log gave.");
    const plans = db
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .select(sql<string>`subscriptions.id::text`.as("id"))
      .where("billing_customers.org_id", "=", orgId)
      .where("subscriptions.product", "=", "team");
    let query = db
      .selectFrom("audit")
      .selectAll()
      .where((eb) =>
        eb.or([
          eb.and([
            eb("subject", "=", orgId),
            eb("action", "in", Object.keys(SHOWN)),
          ]),
          eb.and([
            eb("subject", "in", plans),
            eb("action", "in", Object.keys(PLAN)),
          ]),
        ]),
      );
    if (cursor) query = query.where("id", "<", cursor);
    const rows = await query
      .orderBy("id", "desc")
      .limit(PAGE + 1)
      .execute();
    const page = rows.slice(0, PAGE);
    const named = await people(c, [
      ...new Set(
        page.flatMap((row) =>
          [userOf(row.actor), row.user_id].filter(
            (id): id is string => id !== null,
          ),
        ),
      ),
    ]);
    const body: OrgAuditPage = {
      entries: page.map((row) => entryBody(row, named)),
      next: rows.length > PAGE ? String(page[page.length - 1]?.id) : null,
    };
    return c.json(body);
  });
}
