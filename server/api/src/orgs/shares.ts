import type { Services } from "../app.ts";
import type { ShareRegistry, SharedProject } from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import type { Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ownedDevice } from "../devices/owned.ts";
import { dropUnshared } from "../orgvault/projects.ts";
import { appendEvents } from "../events/log.ts";
import { teamPaid } from "./host.ts";
import { notFound, readBody } from "../http.ts";
import { signPeerTicket } from "../join/signer.ts";
import { perUser } from "../limits.ts";

const NOT_IN_PAID_ORG =
  "You are not in an org with that id whose Team plan is paid.";

/** The account's memberships in orgs whose Team plan is paid. Until plans go live, staff only. */
async function paidMemberships(
  db: Kysely<Tables>,
  userId: string,
  live: boolean,
  orgId?: string,
) {
  if (!live && !(await isStaff(db, userId))) return [];
  let query = db
    .selectFrom("orgs")
    .innerJoin("org_users", "org_users.org_id", "orgs.id")
    .select(["orgs.id as org", "org_users.id as member"])
    .where("org_users.user_id", "=", userId)
    .where(teamPaid);
  if (orgId) query = query.where("orgs.id", "=", orgId);
  return query.orderBy("orgs.created_at").orderBy("orgs.id").execute();
}

/** Tells every member of the org that its shares changed. */
async function sharesChanged(db: Kysely<Tables>, orgId: string) {
  await db.transaction().execute(async (trx) => {
    const members = await trx
      .selectFrom("org_users")
      .select("user_id")
      .where("org_id", "=", orgId)
      .execute();
    await appendEvents(
      trx,
      members.map(({ user_id }) => ({
        userId: user_id,
        type: "shares.changed" as const,
      })),
    );
  });
}

/**
 * The share registry: which projects members share with their orgs, and from which host. Only
 * hosts sign these requests, and only for orgs whose Team plan is paid. A teammate learns a hash
 * of a repository's address and a host key to dial, never the address or the host's name.
 */
export function shareRoutes({
  database: { db },
  verifier,
  limiter,
  clerk,
  join: signer,
  billing: { live },
}: Pick<
  Services,
  "database" | "verifier" | "limiter" | "clerk" | "join" | "billing"
>) {
  const names = async (c: Context<AuthEnv>, members: MemberRow[]) => {
    const people = new Map<
      string,
      { name: string | null; imageUrl: string | null }
    >();
    if (clerk)
      try {
        const userIds = [...new Set(members.map((member) => member.user_id))];
        for (let at = 0; at < userIds.length; at += 100)
          for (const [id, person] of await clerk.people(
            userIds.slice(at, at + 100),
          ))
            people.set(id, {
              name: person.name?.slice(0, 128) || null,
              imageUrl: person.imageUrl ?? null,
            });
      } catch (error) {
        c.get("log").warn(
          { err: error },
          "Clerk could not name an org's members",
        );
      }
    return (member: MemberRow) => {
      const person = people.get(member.user_id);
      return {
        name: person?.name ?? member.id.slice(0, 8),
        imageUrl: person?.imageUrl ?? null,
      };
    };
  };

  const membersOf = (orgIds: string[]) =>
    orgIds.length === 0
      ? Promise.resolve([] as MemberRow[])
      : db
          .selectFrom("org_users")
          .select(["id", "org_id", "user_id"])
          .where("org_id", "in", orgIds)
          .orderBy("joined_at")
          .execute();

  const hostsOnly = requireIdentity(verifier, db, { accept: ["host"] });
  const shares = new Hono<AuthEnv>()
    .use(hostsOnly)
    .get("/", perUser(limiter, "share-registry", 30), async (c) => {
      const { userId } = c.get("identity");
      const memberships = await paidMemberships(db, userId, live);
      const orgIds = memberships.map(({ org }) => org);
      const members = await membersOf(orgIds);
      const rows =
        orgIds.length === 0
          ? []
          : await db
              .selectFrom("org_shares")
              .innerJoin("org_users", "org_users.id", "org_shares.member_id")
              .innerJoin("devices", (join) =>
                join
                  .onRef("devices.key", "=", "org_shares.host")
                  .onRef("devices.user_id", "=", "org_users.user_id"),
              )
              .select([
                "org_users.org_id",
                "org_shares.member_id",
                "org_shares.project",
                "org_shares.host",
              ])
              .where("org_users.org_id", "in", orgIds)
              .orderBy("org_shares.created_at")
              .execute();
      const nameOf = await names(c, members);
      const mine = new Set(memberships.map(({ member }) => member));
      const body: ShareRegistry = {
        orgs: memberships.map(({ org }) => {
          const here = rows
            .filter((share) => share.org_id === org)
            .map((share): SharedProject => ({
              project: share.project,
              member: share.member_id,
              host: share.host,
            }));
          return {
            org,
            members: members
              .filter((member) => member.org_id === org)
              .map((member) => ({
                id: member.id,
                ...nameOf(member),
                you: mine.has(member.id),
              })),
            shares: here.filter((share) => !mine.has(share.member)),
            mine: here.filter((share) => mine.has(share.member)),
          };
        }),
      };
      return c.json(body);
    })
    .put("/", perUser(limiter, "share-register", 30), async (c) => {
      const { userId } = c.get("identity");
      const { org, project, host } = await readBody(c, "ShareRegistration");
      const [membership] = await paidMemberships(db, userId, live, org);
      if (!membership) throw notFound(NOT_IN_PAID_ORG);
      await ownedDevice(db, userId, host, { role: "host" });
      const before = await db
        .selectFrom("org_shares")
        .select("host")
        .where("member_id", "=", membership.member)
        .where("project", "=", project)
        .executeTakeFirst();
      if (before?.host === host) return c.body(null, 204);
      await db
        .insertInto("org_shares")
        .values({ member_id: membership.member, project, host })
        .onConflict((oc) =>
          oc
            .columns(["member_id", "project"])
            .doUpdateSet({ host, updated_at: new Date() }),
        )
        .execute();
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: before ? "share.moved" : "share.added",
        subject: org,
        detail: { host: host.slice(0, 8) },
      });
      await sharesChanged(db, org);
      return c.body(null, 204);
    })
    .post("/remove", perUser(limiter, "share-remove", 30), async (c) => {
      const { userId } = c.get("identity");
      const { org, project } = await readBody(c, "ShareRemoval");
      const removed = await db.transaction().execute(async (trx) => {
        const removed = await trx
          .deleteFrom("org_shares")
          .where("project", "=", project)
          .where("member_id", "in", (eb) =>
            eb
              .selectFrom("org_users")
              .select("id")
              .where("org_id", "=", org)
              .where("user_id", "=", userId),
          )
          .executeTakeFirst();
        if (removed.numDeletedRows > 0n) await dropUnshared(trx, org);
        return removed;
      });
      if (removed.numDeletedRows > 0n) {
        await audit(db, {
          userId,
          actor: `user:${userId}`,
          action: "share.removed",
          subject: org,
        });
        await sharesChanged(db, org);
      }
      return c.body(null, 204);
    });
  const tickets = new Hono<AuthEnv>()
    .use(hostsOnly)
    .post("/", perUser(limiter, "peer-tickets", 30), async (c) => {
      const { userId, deviceKey } = c.get("identity");
      const { org, host } = await readBody(c, "PeerTicketRequest");
      const [membership] = await paidMemberships(db, userId, live, org);
      if (!membership || !deviceKey) throw notFound(NOT_IN_PAID_ORG);
      const target = await db
        .selectFrom("org_shares")
        .innerJoin("org_users", "org_users.id", "org_shares.member_id")
        .innerJoin("devices", (join) =>
          join
            .onRef("devices.key", "=", "org_shares.host")
            .onRef("devices.user_id", "=", "org_users.user_id"),
        )
        .select("org_users.user_id")
        .where("org_users.org_id", "=", org)
        .where("org_shares.host", "=", host)
        .where("org_users.user_id", "!=", userId)
        .executeTakeFirst();
      if (!target)
        throw notFound(
          "No other member of that org shares a project from that host.",
        );
      const self = await db
        .selectFrom("org_users")
        .select(["id", "org_id", "user_id"])
        .where("id", "=", membership.member)
        .executeTakeFirstOrThrow();
      const nameOf = await names(c, [self]);
      const ticket = signPeerTicket(signer, {
        org,
        member: membership.member,
        memberName: nameOf(self).name,
        from: deviceKey,
        to: host,
      });
      c.get("log").info(
        { from: deviceKey.slice(0, 8), to: host.slice(0, 8) },
        "issued a peer ticket",
      );
      return c.json(ticket);
    });
  return { shares, tickets };
}

type MemberRow = Pick<Tables["org_users"], "org_id" | "user_id"> & {
  id: string;
};
