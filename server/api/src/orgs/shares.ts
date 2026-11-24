import type { Services } from "../app.ts";
import type { ShareRegistry, SharedProject } from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import type { Kysely, Transaction } from "kysely";

import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import { mayUseTeam, teamActive } from "../billing/team.ts";
import type { Tables } from "../db.ts";
import { ownedDevice } from "../devices/owned.ts";
import { dropUnshared } from "../orgvault/projects.ts";
import { appendEvents } from "../events/log.ts";
import { conflict, forbidden, notFound, readBody } from "../http.ts";
import { signPeerTicket } from "../join/signer.ts";
import { perUser } from "../limits.ts";

const MEMBER_SHARE_LIMIT = 200;
const NOT_IN_ACTIVE_ORG =
  "You are not in an org with that id that has Team now.";

const isRegisteredMeanwhile = (error: unknown) =>
  (error as { code?: string }).code === "23505";

/** The account's memberships in orgs that have Team now, when Team is open to the account. */
async function activeMemberships(
  db: Kysely<Tables>,
  userId: string,
  orgId?: string,
) {
  if (!(await mayUseTeam(db, userId))) return [];
  let query = db
    .selectFrom("orgs")
    .innerJoin("org_users", "org_users.org_id", "orgs.id")
    .select(["orgs.id as org", "org_users.id as member"])
    .where("org_users.user_id", "=", userId)
    .where(teamActive);
  if (orgId) query = query.where("orgs.id", "=", orgId);
  return query.orderBy("orgs.created_at").orderBy("orgs.id").execute();
}

/** Tells every member of the org that its shares changed. */
async function sharesChanged(trx: Transaction<Tables>, orgId: string) {
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
}

/** The orgs whose memberships are among `members`. */
async function orgsOf(trx: Transaction<Tables>, members: string[]) {
  if (members.length === 0) return [];
  const orgs = await trx
    .selectFrom("org_users")
    .select("org_id")
    .distinct()
    .where("id", "in", members)
    .execute();
  return orgs.map(({ org_id }) => org_id);
}

/** Ends every share a host held, when the host leaves its account. */
export async function endHostShares(trx: Transaction<Tables>, hostKey: string) {
  const ended = await trx
    .deleteFrom("org_shares")
    .where("host", "=", hostKey)
    .returning("member_id")
    .execute();
  for (const org of await orgsOf(
    trx,
    ended.map(({ member_id }) => member_id),
  )) {
    await dropUnshared(trx, org);
    await sharesChanged(trx, org);
  }
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
}: Pick<Services, "database" | "verifier" | "limiter" | "clerk" | "join">) {
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
      const memberships = await activeMemberships(db, userId);
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
      const [membership] = await activeMemberships(db, userId, org);
      if (!membership) throw notFound(NOT_IN_ACTIVE_ORG);
      await ownedDevice(db, userId, host, { role: "host" });
      const before = await db
        .selectFrom("org_shares")
        .select("host")
        .where("member_id", "=", membership.member)
        .where("project", "=", project)
        .executeTakeFirst();
      if (!before) {
        const { count } = await db
          .selectFrom("org_shares")
          .select((eb) => eb.fn.countAll<string>().as("count"))
          .where("member_id", "=", membership.member)
          .executeTakeFirstOrThrow();
        if (Number(count) >= MEMBER_SHARE_LIMIT) {
          throw conflict(
            `You already share ${MEMBER_SHARE_LIMIT} projects with this org, the most one person can. Stop sharing one to share another.`,
          );
        }
      }
      const registered = db.transaction().execute(async (trx) => {
        const elsewhere = await trx
          .deleteFrom("org_shares")
          .where("user_id", "=", userId)
          .where("project", "=", project)
          .where("member_id", "!=", membership.member)
          .returning("member_id")
          .execute();
        for (const left of await orgsOf(
          trx,
          elsewhere.map(({ member_id }) => member_id),
        )) {
          await dropUnshared(trx, left);
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "share.removed",
            subject: left,
          });
          await sharesChanged(trx, left);
        }
        const tapped_at = new Date();
        await trx
          .insertInto("org_shares")
          .values({
            member_id: membership.member,
            user_id: userId,
            project,
            host,
            tapped_at,
          })
          .onConflict((oc) =>
            oc
              .columns(["member_id", "project"])
              .doUpdateSet({ host, tapped_at, updated_at: tapped_at }),
          )
          .execute();
        if (before?.host === host) return;
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: before ? "share.moved" : "share.added",
          subject: org,
          detail: { host: host.slice(0, 8) },
        });
        await sharesChanged(trx, org);
      });
      await registered.catch((error: unknown) => {
        if (isRegisteredMeanwhile(error))
          throw conflict(
            "Another of your computers shared this project at the same moment. Try again.",
          );
        throw error;
      });
      return c.body(null, 204);
    })
    .post("/move", perUser(limiter, "share-move", 30), async (c) => {
      const { userId } = c.get("identity");
      const { org, project, host } = await readBody(c, "ShareMove");
      const [membership] = await activeMemberships(db, userId, org);
      if (!membership) throw notFound(NOT_IN_ACTIVE_ORG);
      await ownedDevice(db, userId, host, { role: "host" });
      await db.transaction().execute(async (trx) => {
        const moved = await trx
          .updateTable("org_shares")
          .set({ host, updated_at: new Date() })
          .where("member_id", "=", membership.member)
          .where("project", "=", project)
          .executeTakeFirst();
        if (moved.numUpdatedRows === 0n)
          throw conflict(
            "You no longer share that project with the org, so there is nothing to move.",
          );
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "share.moved",
          subject: org,
          detail: { host: host.slice(0, 8) },
        });
        await sharesChanged(trx, org);
      });
      return c.body(null, 204);
    })
    .post("/rename", perUser(limiter, "share-rename", 30), async (c) => {
      const { userId, deviceKey } = c.get("identity");
      const { from, project } = await readBody(c, "ShareRename");
      await db.transaction().execute(async (trx) => {
        const moved = await trx
          .deleteFrom("org_shares")
          .where("user_id", "=", userId)
          .where("project", "=", from)
          .where("host", "=", deviceKey ?? "")
          .returning(["member_id", "host", "tapped_at"])
          .execute();
        const [tapped] = moved;
        if (!tapped) {
          const held = await trx
            .selectFrom("org_shares")
            .select("host")
            .where("user_id", "=", userId)
            .where("project", "=", from)
            .executeTakeFirst();
          if (held)
            throw forbidden(
              "Another of your computers holds that share, and moves it once it sees the rename.",
            );
          throw conflict(
            "You share that project with no org, so there is nothing to move.",
          );
        }
        const already = await trx
          .selectFrom("org_shares")
          .select(["member_id", "tapped_at"])
          .where("user_id", "=", userId)
          .where("project", "=", project)
          .executeTakeFirst();
        const wins = !already || already.tapped_at < tapped.tapped_at;
        if (wins) {
          if (already)
            await trx
              .deleteFrom("org_shares")
              .where("member_id", "=", already.member_id)
              .where("project", "=", project)
              .execute();
          await trx
            .insertInto("org_shares")
            .values({ ...tapped, user_id: userId, project })
            .execute();
        }
        const holding = already && !wins ? already.member_id : tapped.member_id;
        const ended = [tapped.member_id, already?.member_id].filter(
          (member): member is string => !!member && member !== holding,
        );
        for (const org of await orgsOf(trx, ended)) {
          await dropUnshared(trx, org);
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "share.removed",
            subject: org,
          });
          await sharesChanged(trx, org);
        }
        if (!wins) return;
        for (const org of await orgsOf(trx, [holding])) {
          await dropUnshared(trx, org);
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "share.moved",
            subject: org,
          });
          await sharesChanged(trx, org);
        }
      });
      return c.body(null, 204);
    })
    .post("/remove", perUser(limiter, "share-remove", 30), async (c) => {
      const { userId } = c.get("identity");
      const { org, project } = await readBody(c, "ShareRemoval");
      await db.transaction().execute(async (trx) => {
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
        if (removed.numDeletedRows > 0n) {
          await dropUnshared(trx, org);
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "share.removed",
            subject: org,
          });
          await sharesChanged(trx, org);
        }
      });
      return c.body(null, 204);
    });
  const tickets = new Hono<AuthEnv>()
    .use(hostsOnly)
    .post("/", perUser(limiter, "peer-tickets", 30), async (c) => {
      const { userId, deviceKey } = c.get("identity");
      const { org, host } = await readBody(c, "PeerTicketRequest");
      const [membership] = await activeMemberships(db, userId, org);
      if (!membership || !deviceKey) throw notFound(NOT_IN_ACTIVE_ORG);
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
