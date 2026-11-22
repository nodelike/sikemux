import { createHash } from "node:crypto";
import type {
  WorkAddress,
  WorkAddressList,
  WorkAddressProven,
  WorkAddressVerifyStart,
} from "@sikemux/protocol";
import { Hono } from "hono";
import { sql, type Kysely } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ApiFailure, conflict, notFound, readBody } from "../http.ts";
import { perUser } from "../limits.ts";
import { offersFor } from "../orgs/domains.ts";
import { isUuid } from "../orgs/shared.ts";
import { addressHash, domainOf } from "./proofs.ts";

/** Where Google sends the person back to: the web app, which finishes on their own session. */
export const WORK_ADDRESS_RETURN = "/work-address";

const NO_SITTING =
  "That verifying was started from another account or over ten minutes ago. Start again from Work addresses.";

const stateHash = (state: string) =>
  createHash("sha256").update(state).digest();

async function proofsOf(
  db: Kysely<Tables>,
  userId: string,
  hash?: Buffer,
): Promise<WorkAddress[]> {
  let rows = db
    .selectFrom("address_proofs")
    .leftJoin("orgs", "orgs.id", "address_proofs.org_id")
    .select([
      "address_proofs.id",
      "address_proofs.domain",
      "address_proofs.org_id",
      "address_proofs.proven_at",
      "orgs.name",
    ])
    .where("address_proofs.user_id", "=", userId)
    .orderBy("address_proofs.proven_at");
  if (hash) rows = rows.where("address_proofs.address_hash", "=", hash);
  return (await rows.execute()).map((row) => ({
    id: row.id,
    domain: row.domain,
    orgId: row.name ? row.org_id : null,
    orgName: row.name,
    provenAt: row.proven_at.toISOString(),
  }));
}

/**
 * The person's proven work addresses, and proving one by signing in to its Google account. The
 * address is read once to hash it and never kept or logged.
 */
export function workAddressRoutes({
  database: { db },
  verifier,
  limiter,
  appOrigin,
  addressProofKey,
  workGoogle,
}: Pick<
  Services,
  | "database"
  | "verifier"
  | "limiter"
  | "appOrigin"
  | "addressProofKey"
  | "workGoogle"
>) {
  const redirectUri = `${appOrigin}${WORK_ADDRESS_RETURN}`;
  const ready = () => {
    if (!workGoogle || !addressProofKey)
      throw new ApiFailure(
        503,
        "unavailable",
        "Verifying work addresses isn't set up here.",
      );
    return { google: workGoogle, key: addressProofKey };
  };

  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db))
    .get("/", async (c) => {
      const body: WorkAddressList = {
        workAddresses: await proofsOf(db, c.get("identity").userId),
        verifying: Boolean(workGoogle && addressProofKey),
      };
      return c.json(body);
    })
    .post("/verify", perUser(limiter, "work-address-verify", 20), async (c) => {
      const { userId } = c.get("identity");
      const { google } = ready();
      const started = google.start(redirectUri);
      await db
        .deleteFrom("work_address_sittings")
        .where("user_id", "=", userId)
        .where("created_at", "<", sql<Date>`now() - interval '1 day'`)
        .execute();
      await db
        .insertInto("work_address_sittings")
        .values({
          user_id: userId,
          state_hash: stateHash(started.state),
          nonce: started.nonce,
          verifier: started.verifier,
        })
        .execute();
      const body: WorkAddressVerifyStart = { url: started.url };
      return c.json(body);
    })
    .post(
      "/verify/finish",
      perUser(limiter, "work-address-verify", 20),
      async (c) => {
        const { userId } = c.get("identity");
        const { code, state } = await readBody(c, "WorkAddressVerifyFinish");
        const { google, key } = ready();
        const sitting = await db
          .selectFrom("work_address_sittings")
          .select(["id", "nonce", "verifier"])
          .where("state_hash", "=", stateHash(state))
          .where("user_id", "=", userId)
          .where("proven_at", "is", null)
          .where("created_at", ">", sql<Date>`now() - interval '10 minutes'`)
          .executeTakeFirst();
        if (!sitting) throw notFound(NO_SITTING);
        const email = await google.address({
          code,
          verifier: sitting.verifier,
          nonce: sitting.nonce,
          redirectUri,
        });
        const domain = domainOf(email);
        if (!domain)
          throw conflict(
            "That address is a test address, so it can't be verified as a work address. Verify your own work address instead.",
          );
        const hash = addressHash(key, email);
        const proven = await db.transaction().execute(async (trx) => {
          const org = await trx
            .selectFrom("org_domains")
            .innerJoin("orgs", "orgs.id", "org_domains.org_id")
            .select(["orgs.id", "orgs.name"])
            .where("org_domains.domain", "=", domain)
            .where("org_domains.verified_at", "is not", null)
            .executeTakeFirst();
          if (!org)
            throw conflict(
              `No org on Sikemux has verified ${domain}, so an address there isn't a work address here.`,
            );
          const { rows } = await sql`
            insert into address_proofs (address_hash, user_id, domain, org_id, via)
            values (${hash}, ${userId}, ${domain}, ${org.id}, 'google')
            on conflict (address_hash) do update
              set domain = excluded.domain, org_id = excluded.org_id, via = 'google', proven_at = now()
              where address_proofs.user_id = excluded.user_id
            returning 1`.execute(trx);
          if (rows.length === 0)
            throw conflict(
              "This address is already verified on another Sikemux account, so it can't be verified on this one too.",
            );
          const marked = await trx
            .updateTable("work_address_sittings")
            .set({ proven_at: new Date(), address_hash: hash })
            .where("id", "=", sitting.id)
            .where("proven_at", "is", null)
            .executeTakeFirst();
          if (marked.numUpdatedRows === 0n) throw notFound(NO_SITTING);
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "account.work_address_proven",
            detail: { domain, orgId: org.id },
          });
          return org;
        });
        c.get("log").info(
          { domain, orgId: proven.id },
          "proved a work address",
        );
        const [workAddress] = await proofsOf(db, userId, hash);
        if (!workAddress) throw notFound(NO_SITTING);
        const offers = await offersFor(db, userId, [domain]);
        const body: WorkAddressProven = {
          workAddress,
          sitting: sitting.id,
          offer: offers.find((offer) => offer.orgId === proven.id) ?? null,
        };
        return c.json(body);
      },
    )
    .delete("/:id", async (c) => {
      const { userId } = c.get("identity");
      const id = c.req.param("id");
      if (!isUuid(id)) return c.body(null, 204);
      await db.transaction().execute(async (trx) => {
        const gone = await trx
          .deleteFrom("address_proofs")
          .where("id", "=", id)
          .where("user_id", "=", userId)
          .returning("domain")
          .executeTakeFirst();
        if (gone)
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "account.work_address_removed",
            detail: { domain: gone.domain },
          });
      });
      return c.body(null, 204);
    });
}
