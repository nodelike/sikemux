import type { Services } from "../app.ts";
import type { Transaction } from "kysely";
import { Hono, type Context } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import {
  badRequest,
  forbidden,
  limitBody,
  notFound,
  readBody,
} from "../http.ts";
import { perUser } from "../limits.ts";
import {
  addOrgKeys,
  createOrgVault,
  putOrgEntry,
  remakeOrgVault,
  removeOrgEntry,
  rotateOrgVault,
  type Asker,
} from "./change.ts";
import { allowPut } from "./projects.ts";
import { memberOf, NOT_IN_ORG, orgVaultState, type Member } from "./state.ts";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const ENTRY_ID = /^[0-9a-f]{32}$/;
const REVISION = /^[1-9][0-9]{0,8}$/;
const CHANGES = /^(0|[1-9][0-9]{0,14})$/;

/** The most one sealed value takes in a body, with room for the fields around it. */
const SEALED_BYTES = 87_400 + 256;
/** The most one wrapped key takes in a body, with its host's key and the field names. */
const WRAP_BYTES = 256 + 128;

type Need = "member" | "paid member" | "manager" | "paid manager";

function entryId(c: Context): string {
  const id = c.req.param("id") ?? "";
  if (!ENTRY_ID.test(id))
    throw badRequest("A vault entry's id is 32 lowercase hex characters.");
  return id;
}

function allow(member: Member, need: Need) {
  if (need === "member") return;
  if (need !== "paid member" && member.role === "user")
    throw forbidden("Only an org's owner or admin may change its vault.");
  if (need !== "manager" && !member.active)
    throw forbidden(
      "The org's Team plan has lapsed: its vault keeps what it holds and takes nothing new.",
    );
}

/**
 * An org's vault: what its members' hosts share, sealed under a key only they hold. The server
 * keeps the sealed entries and the key sealed for each host, and reads neither. Only an owner's
 * or admin's host writes to it, but for a shared project's rules (see projects.ts).
 */
export function orgVaultRoutes({
  database: { db },
  verifier,
  limiter,
  presence,
  billing: { live },
}: Pick<
  Services,
  "database" | "verifier" | "limiter" | "presence" | "billing"
>) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });

  const asker = (c: Context<AuthEnv>): Asker => {
    const orgId = c.req.param("orgId") ?? "";
    if (!UUID.test(orgId)) throw notFound(NOT_IN_ORG);
    const { userId, deviceKey } = c.get("identity");
    if (!deviceKey) throw forbidden("Only a host holds an org's vault.");
    return { orgId, userId, hostKey: deviceKey };
  };

  /** Runs `change` in one transaction once the caller's role in the org allows it. */
  const changing = <T>(
    c: Context<AuthEnv>,
    need: Need,
    change: (
      trx: Transaction<Tables>,
      asker: Asker,
      member: Member,
    ) => Promise<T>,
  ) => {
    const who = asker(c);
    return db.transaction().execute(async (trx) => {
      const member = await memberOf(trx, live, who.orgId, who.userId);
      allow(member, need);
      return change(trx, who, member);
    });
  };

  /** Runs `change` as `changing` does, then answers the vault as the caller's host now sees it. */
  const changed = async (
    c: Context<AuthEnv>,
    need: Need,
    change: (
      trx: Transaction<Tables>,
      asker: Asker,
      member: Member,
    ) => Promise<void>,
  ) => {
    const member = await changing(c, need, async (trx, who, member) => {
      await change(trx, who, member);
      return member;
    });
    const { orgId, userId, hostKey } = asker(c);
    return orgVaultState(
      db,
      orgId,
      { hostKey, userId, role: member.role },
      presence,
    );
  };

  return new Hono<AuthEnv>()
    .get("/", asHost, perUser(limiter, "org-vault-get", 240), async (c) => {
      const { orgId, userId, hostKey } = asker(c);
      const { role } = await memberOf(db, live, orgId, userId);
      const after = c.req.query("after");
      if (after !== undefined && !CHANGES.test(after))
        throw badRequest("after is a count of the vault's changes.");
      return c.json(
        await orgVaultState(
          db,
          orgId,
          { hostKey, userId, role },
          presence,
          after === undefined ? undefined : Number(after),
        ),
      );
    })
    .post(
      "/",
      limitBody<AuthEnv>(4 * WRAP_BYTES),
      asHost,
      perUser(limiter, "org-vault-create", 10),
      async (c) => {
        const body = await readBody(c, "OrgVaultCreate");
        return c.json(
          await changed(c, "paid manager", (trx, who) =>
            createOrgVault(trx, who, body),
          ),
          201,
        );
      },
    )
    .put(
      "/keys",
      limitBody<AuthEnv>(501 * WRAP_BYTES),
      asHost,
      perUser(limiter, "org-vault-keys", 60),
      async (c) => {
        const body = await readBody(c, "OrgVaultKeysAdd");
        return c.json(
          await changed(c, "member", (trx, who, member) =>
            addOrgKeys(trx, who, member.role, body),
          ),
        );
      },
    )
    .post(
      "/rotate",
      limitBody<AuthEnv>(501 * WRAP_BYTES + 501 * SEALED_BYTES),
      asHost,
      perUser(limiter, "org-vault-rotate", 10),
      async (c) => {
        const body = await readBody(c, "OrgVaultRotate");
        return c.json(
          await changed(c, "manager", (trx, who) =>
            rotateOrgVault(trx, who, body),
          ),
        );
      },
    )
    .post(
      "/remake",
      limitBody<AuthEnv>(4 * WRAP_BYTES),
      asHost,
      perUser(limiter, "org-vault-remake", 10),
      async (c) => {
        const body = await readBody(c, "OrgVaultRemake");
        return c.json(
          await changed(c, "paid manager", (trx, who) =>
            remakeOrgVault(trx, who, body),
          ),
        );
      },
    )
    .put(
      "/entries/:id",
      limitBody<AuthEnv>(2 * SEALED_BYTES),
      asHost,
      perUser(limiter, "org-vault-put", 120),
      async (c) => {
        const id = entryId(c);
        const body = await readBody(c, "OrgVaultEntryPut");
        return c.json(
          await changing(c, "paid member", async (trx, who, member) => {
            await allowPut(trx, who.orgId, member, id, body.kind);
            return putOrgEntry(trx, who, id, body);
          }),
        );
      },
    )
    .delete(
      "/entries/:id",
      asHost,
      perUser(limiter, "org-vault-delete", 120),
      async (c) => {
        const id = entryId(c);
        const revision = c.req.query("revision") ?? "";
        if (!REVISION.test(revision))
          throw badRequest(
            "Send the entry's current revision as ?revision=<number>.",
          );
        await changing(c, "manager", (trx, who) =>
          removeOrgEntry(trx, who, id, Number(revision)),
        );
        return c.body(null, 204);
      },
    );
}
