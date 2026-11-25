import { Hono, type Context } from "hono";
import type { Transaction } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import { requireTeam } from "../billing/team.ts";
import type { OrgLogoMime, Tables } from "../db.ts";
import { badRequest, limitBody, notFound } from "../http.ts";
import { perUser } from "../limits.ts";
import {
  membership,
  membersOf,
  orgBody,
  orgIdOf,
  orgsChanged,
  requireRole,
  requireTeamPlan,
} from "./shared.ts";

/** Under the 1 MiB the proxy lets through. */
const LOGO_BYTES = 1_000_000;

/** What the picture is by its own first bytes, whatever the request calls it. */
function pictureType(bytes: Buffer): OrgLogoMime | undefined {
  if (bytes.subarray(0, 8).toString("hex") === "89504e470d0a1a0a")
    return "image/png";
  if (bytes.subarray(0, 3).toString("hex") === "ffd8ff") return "image/jpeg";
  if (
    bytes.subarray(0, 4).toString("latin1") === "RIFF" &&
    bytes.subarray(8, 12).toString("latin1") === "WEBP"
  )
    return "image/webp";
  return undefined;
}

/** An org's logo: its members read it, and its owner or an admin sets or removes it. */
export function orgLogoRoutes({
  database: { db },
  verifier,
  limiter,
  billing: { live },
}: Pick<Services, "database" | "verifier" | "limiter" | "billing">) {
  /** Runs `change` once the caller may, and answers the org as it is then. */
  const changing = async (
    c: Context<AuthEnv>,
    change: (
      trx: Transaction<Tables>,
      orgId: string,
    ) => Promise<"org.logo_changed" | "org.logo_removed" | null>,
  ) => {
    const { userId } = c.get("identity");
    const orgId = orgIdOf(c);
    const { org, role } = await db.transaction().execute(async (trx) => {
      const member = await membership(trx, orgId, userId, { lock: true });
      requireRole(member.role, ["owner", "admin"]);
      await requireTeamPlan(trx, orgId);
      const action = await change(trx, orgId);
      if (action) {
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action,
          subject: orgId,
        });
        await orgsChanged(trx, await membersOf(trx, orgId));
      }
      return member;
    });
    return c.json(await orgBody(db, org, role));
  };

  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db))
    .use(requireTeam(db, live))
    .get("/", async (c) => {
      const orgId = orgIdOf(c);
      await membership(db, orgId, c.get("identity").userId);
      const logo = await db
        .selectFrom("org_logos")
        .selectAll()
        .where("org_id", "=", orgId)
        .executeTakeFirst();
      if (!logo) throw notFound("The org has no logo.");
      const headers = {
        etag: `"${logo.id}"`,
        "cache-control": "private, no-cache",
      };
      if (c.req.header("if-none-match") === headers.etag)
        return c.body(null, 304, headers);
      return c.body(new Uint8Array(logo.bytes), 200, {
        ...headers,
        "content-type": logo.mime,
        "x-content-type-options": "nosniff",
        "content-security-policy": "sandbox; default-src 'none'",
      });
    })
    .put(
      "/",
      perUser(limiter, "org-logo", 30),
      limitBody<AuthEnv>(LOGO_BYTES),
      async (c) => {
        const bytes = Buffer.from(await c.req.arrayBuffer());
        const mime = pictureType(bytes);
        if (!mime)
          throw badRequest("An org's logo is a PNG, JPEG or WebP picture.");
        return changing(c, async (trx, orgId) => {
          await trx
            .deleteFrom("org_logos")
            .where("org_id", "=", orgId)
            .execute();
          await trx
            .insertInto("org_logos")
            .values({ org_id: orgId, mime, bytes })
            .execute();
          return "org.logo_changed";
        });
      },
    )
    .delete("/", perUser(limiter, "org-logo", 30), (c) =>
      changing(c, async (trx, orgId) => {
        const { numDeletedRows } = await trx
          .deleteFrom("org_logos")
          .where("org_id", "=", orgId)
          .executeTakeFirst();
        return numDeletedRows > 0n ? "org.logo_removed" : null;
      }),
    );
}
