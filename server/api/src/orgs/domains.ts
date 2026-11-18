import { randomBytes } from "node:crypto";
import type {
  OrgDomain,
  OrgDomainList,
  OrgOffer,
  OrgOfferList,
} from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Selectable } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import type { AuthEnv } from "../auth.ts";
import { seatLimit, seatsHeld } from "../billing/seats.ts";
import type { Tables } from "../db.ts";
import { badRequest, conflict, notFound, readBody } from "../http.ts";
import { perUser, type RateLimiter } from "../limits.ts";
import {
  admit,
  idParam,
  isUuid,
  membership,
  orgBody,
  orgIdOf,
  requireRole,
  requireTeamPlan,
} from "./shared.ts";

const NO_DOMAIN = "The org has no domain with that id.";
const DOMAIN =
  /^(?=.{4,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const RECORD_PREFIX = "_sikemux-verify";
const VALUE_PREFIX = "sikemux-verify=";

/** What the person typed, as a bare lowercase domain: "@Example.com." is example.com. */
function domainName(typed: string) {
  const domain = typed
    .trim()
    .toLowerCase()
    .replace(/^@/, "")
    .replace(/\.$/, "");
  if (!DOMAIN.test(domain))
    throw badRequest(`${JSON.stringify(typed)} is not an email domain.`);
  return domain;
}

function domainBody(row: Selectable<Tables["org_domains"]>): OrgDomain {
  return {
    id: row.id,
    domain: row.domain,
    joining: row.joining,
    record: {
      name: `${RECORD_PREFIX}.${row.domain}`,
      value: `${VALUE_PREFIX}${row.token}`,
    },
    verifiedAt: row.verified_at?.toISOString() ?? null,
    oursSince: row.ours_since?.toISOString() ?? null,
    createdAt: row.created_at.toISOString(),
  };
}

const isTaken = (error: unknown) =>
  (error as { code?: string }).code === "23505";

/** The domains of the person's verified addresses, read now and never kept. */
async function emailDomains(clerk: ClerkBackend | null, userId: string) {
  return clerk ? await clerk.verifiedDomains(userId) : [];
}

/** The orgs not yet joined whose verified domain is one of `domains`, leaving out those with no seat to give. */
export async function offersFor(
  db: Kysely<Tables>,
  userId: string,
  domains: string[],
): Promise<OrgOffer[]> {
  if (!domains.length) return [];
  const rows = await db
    .selectFrom("org_domains")
    .innerJoin("orgs", "orgs.id", "org_domains.org_id")
    .select([
      "orgs.id as orgId",
      "orgs.name",
      "org_domains.domain",
      "org_domains.joining",
    ])
    .where("org_domains.domain", "in", domains)
    .where("org_domains.verified_at", "is not", null)
    .where(({ not, exists, selectFrom }) =>
      not(
        exists(
          selectFrom("org_users")
            .select("org_users.user_id")
            .whereRef("org_users.org_id", "=", "orgs.id")
            .where("org_users.user_id", "=", userId),
        ),
      ),
    )
    .orderBy("orgs.name")
    .execute();
  const offers: OrgOffer[] = [];
  for (const row of rows) {
    const limit = await seatLimit(db, row.orgId);
    if (limit === 0) continue;
    offers.push({ ...row, seatFree: (await seatsHeld(db, row.orgId)) < limit });
  }
  return offers;
}

/**
 * The domain of the work address proven in `sitting`, used up by this join. Only a proof made in
 * that sitting on this account, under half an hour ago, labelled for this org, counts.
 */
async function sittingDomains(
  trx: Kysely<Tables>,
  userId: string,
  orgId: string,
  sitting: string,
): Promise<string[]> {
  const row = await trx
    .selectFrom("work_address_sittings as s")
    .innerJoin("address_proofs as p", "p.address_hash", "s.address_hash")
    .select(["s.id", "p.domain"])
    .where("s.id", "=", sitting)
    .where("s.user_id", "=", userId)
    .where("s.joined_at", "is", null)
    .where("s.proven_at", ">", sql<Date>`now() - interval '30 minutes'`)
    .where("p.user_id", "=", userId)
    .where("p.org_id", "=", orgId)
    .forUpdate("s")
    .executeTakeFirst();
  if (!row) return [];
  await trx
    .updateTable("work_address_sittings")
    .set({ joined_at: new Date() })
    .where("id", "=", row.id)
    .execute();
  return [row.domain];
}

/** Email domains an org proves it owns, and joining an org through one. */
export function domainRoutes(
  db: Kysely<Tables>,
  limiter: RateLimiter,
  clerk: ClerkBackend | null,
  txtRecords: Services["txtRecords"],
  callProvider: (c: Context<AuthEnv>) => Promise<void>,
) {
  const managing = async (
    c: Context<AuthEnv>,
    trx: Kysely<Tables>,
    orgId: string,
    { live = true } = {},
  ) => {
    const { role } = await membership(trx, orgId, c.get("identity").userId, {
      lock: true,
    });
    requireRole(role, ["owner", "admin"]);
    if (live) await requireTeamPlan(trx, orgId);
  };

  const domainOf = async (c: Context, trx: Kysely<Tables>, orgId: string) => {
    const row = await trx
      .selectFrom("org_domains")
      .selectAll()
      .where("org_id", "=", orgId)
      .where("id", "=", idParam(c, "domainId", NO_DOMAIN))
      .executeTakeFirst();
    if (!row) throw notFound(NO_DOMAIN);
    return row;
  };

  const recordFound = async (row: Selectable<Tables["org_domains"]>) => {
    const { name, value } = domainBody(row).record;
    try {
      return (await txtRecords(name)).includes(value);
    } catch {
      return false;
    }
  };

  return new Hono<AuthEnv>()
    .get("/offers", perUser(limiter, "org-offers", 60), async (c) => {
      const { userId } = c.get("identity");
      const body: OrgOfferList = {
        offers: await offersFor(db, userId, await emailDomains(clerk, userId)),
      };
      return c.json(body);
    })
    .post("/offers/join", perUser(limiter, "join-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const { orgId, sitting } = await readBody(c, "OrgDomainJoin");
      const NOT_YOURS = sitting
        ? "The work address you verified is not at a domain that org proved it owns, or you verified it over half an hour ago. Verify it again."
        : "None of your verified email addresses is at a domain that org proved it owns.";
      if (!isUuid(orgId) || (sitting && !isUuid(sitting)))
        throw notFound(NOT_YOURS);
      const org = await db.transaction().execute(async (trx) => {
        const domains = sitting
          ? await sittingDomains(trx, userId, orgId, sitting)
          : await emailDomains(clerk, userId);
        if (!domains.length) throw notFound(NOT_YOURS);
        const claim = await trx
          .selectFrom("org_domains")
          .select("domain")
          .where("org_id", "=", orgId)
          .where("domain", "in", domains)
          .where("verified_at", "is not", null)
          .orderBy("domain")
          .executeTakeFirst();
        if (!claim) throw notFound(NOT_YOURS);
        return admit(trx, {
          orgId,
          userId,
          role: "user",
          invitedBy: null,
          detail: { via: "domain", domain: claim.domain },
        });
      });
      await callProvider(c);
      c.get("log").info({ orgId }, "joined an org by its domain");
      return c.json(await orgBody(db, org, "user"));
    })
    .get("/:orgId/domains", async (c) => {
      const orgId = orgIdOf(c);
      const { role } = await membership(db, orgId, c.get("identity").userId);
      requireRole(role, ["owner", "admin"]);
      const rows = await db
        .selectFrom("org_domains")
        .selectAll()
        .where("org_id", "=", orgId)
        .orderBy("created_at")
        .orderBy("id")
        .execute();
      const body: OrgDomainList = { domains: rows.map(domainBody) };
      return c.json(body);
    })
    .post("/:orgId/domains", perUser(limiter, "org-domain", 30), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const asked = await readBody(c, "OrgDomainAdd");
      const domain = domainName(asked.domain);
      const row = await db
        .transaction()
        .execute(async (trx) => {
          await managing(c, trx, orgId);
          const created = await trx
            .insertInto("org_domains")
            .values({
              org_id: orgId,
              domain,
              joining: asked.joining,
              token: randomBytes(16).toString("base64url"),
              added_by: userId,
            })
            .returningAll()
            .executeTakeFirstOrThrow();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.domain_added",
            subject: orgId,
            detail: { domainId: created.id, domain, joining: asked.joining },
          });
          return created;
        })
        .catch((error: unknown) => {
          if (isTaken(error))
            throw conflict(`The org claims ${domain} already.`);
          throw error;
        });
      return c.json(domainBody(row), 201);
    })
    .post(
      "/:orgId/domains/:domainId/verify",
      perUser(limiter, "org-domain-verify", 20),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { role } = await membership(db, orgId, userId);
        requireRole(role, ["owner", "admin"]);
        const asked = await domainOf(c, db, orgId);
        const found = asked.verified_at !== null || (await recordFound(asked));
        const row = await db
          .transaction()
          .execute(async (trx) => {
            await managing(c, trx, orgId);
            const row = await domainOf(c, trx, orgId);
            if (row.verified_at) return row;
            if (!found || row.token !== asked.token)
              throw conflict(
                `No TXT record at ${domainBody(row).record.name} says ${domainBody(row).record.value} yet. DNS changes can take a while to show; try again in a few minutes.`,
              );
            const verified = await trx
              .updateTable("org_domains")
              .set({ verified_at: new Date() })
              .where("id", "=", row.id)
              .returningAll()
              .executeTakeFirstOrThrow();
            await audit(trx, {
              userId,
              actor: `user:${userId}`,
              action: "org.domain_verified",
              subject: orgId,
              detail: { domainId: row.id, domain: row.domain },
            });
            return verified;
          })
          .catch((error: unknown) => {
            if (isTaken(error))
              throw conflict(
                "Another org verified this domain first. Ask its owner to give it up.",
              );
            throw error;
          });
        return c.json(domainBody(row));
      },
    )
    .put(
      "/:orgId/domains/:domainId/joining",
      perUser(limiter, "org-domain", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { joining } = await readBody(c, "OrgDomainJoiningChange");
        const row = await db.transaction().execute(async (trx) => {
          await managing(c, trx, orgId);
          const row = await domainOf(c, trx, orgId);
          if (row.joining === joining) return row;
          const changed = await trx
            .updateTable("org_domains")
            .set({ joining })
            .where("id", "=", row.id)
            .returningAll()
            .executeTakeFirstOrThrow();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.domain_joining_changed",
            subject: orgId,
            detail: { domainId: row.id, domain: row.domain, joining },
          });
          return changed;
        });
        return c.json(domainBody(row));
      },
    )
    .put(
      "/:orgId/domains/:domainId/ours",
      perUser(limiter, "org-domain", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { ours } = await readBody(c, "OrgDomainOursChange");
        const row = await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          requireRole(role, ["owner"]);
          if (ours) await requireTeamPlan(trx, orgId);
          const row = await domainOf(c, trx, orgId);
          if (ours === (row.ours_since !== null)) return row;
          if (!row.verified_at)
            throw conflict(
              `Verify ${row.domain} first: until then nobody has shown the org owns it.`,
            );
          const changed = await trx
            .updateTable("org_domains")
            .set({ ours_since: ours ? new Date() : null })
            .where("id", "=", row.id)
            .returningAll()
            .executeTakeFirstOrThrow();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.domain_ours_changed",
            subject: orgId,
            detail: { domainId: row.id, domain: row.domain, ours },
          });
          return changed;
        });
        return c.json(domainBody(row));
      },
    )
    .delete(
      "/:orgId/domains/:domainId",
      perUser(limiter, "org-domain", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        await db.transaction().execute(async (trx) => {
          await managing(c, trx, orgId, { live: false });
          const row = await domainOf(c, trx, orgId);
          await trx
            .deleteFrom("org_domains")
            .where("id", "=", row.id)
            .execute();
          await trx
            .deleteFrom("address_proofs")
            .where("org_id", "=", orgId)
            .where("domain", "=", row.domain)
            .execute();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.domain_removed",
            subject: orgId,
            detail: { domainId: row.id, domain: row.domain },
          });
        });
        return c.body(null, 204);
      },
    );
}
