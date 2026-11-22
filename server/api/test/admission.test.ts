import { afterAll, beforeAll, beforeEach, expect, it } from "vitest";

import { admittingVerifier } from "../src/account/admission.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { caller, emptyTables, migratedDatabase } from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { body, log, testApp } from "./support.ts";
import { sessionToken, verifier } from "./tokens.ts";

const OWNER = "user_owner";
const NEWCOMER = "user_newcomer";

let database: Database;
let drop: () => Promise<void>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs cascade");
});

afterAll(() => drop());

/** A new account whose only way in is a Google sign-up Clerk just made from a sign-in it didn't know. */
async function googleSignUpAt(domain: string) {
  const hourAgo = new Date(Date.now() - 3_600_000);
  await database.db
    .insertInto("users")
    .values({ id: OWNER, admitted_at: hourAgo })
    .execute();
  const { id: orgId } = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: OWNER })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_domains")
    .values({
      org_id: orgId,
      domain: "acme.example",
      token: "a".repeat(22),
      added_by: OWNER,
      joining: "offered",
      verified_at: hourAgo,
      ours_since: hourAgo,
    })
    .execute();

  const clerk = new FakeClerk();
  const now = new Date();
  clerk.signUps.set(NEWCOMER, {
    createdAt: now,
    email: `newcomer@${domain}`,
    addresses: [{ email: `newcomer@${domain}`, verified: true, addedAt: now }],
  });
  const app = testApp(database, new RateLimiter(), {
    clerk,
    verifier: admittingVerifier(verifier, database.db, clerk, null, log),
  });
  return caller(app);
}

it("refuses a brand-new Google account at a domain an org claimed", async () => {
  const call = await googleSignUpAt("acme.example");

  const response = await call("/v1/devices", await sessionToken(NEWCOMER));

  expect(response.status).toBe(403);
  const { error } = await body(response, "ApiError");
  expect(error.code).toBe("address_proven");
  expect(error.message).toContain("belong to Acme");
  const admitted = await database.db
    .selectFrom("users")
    .select("admitted_at")
    .where("id", "=", NEWCOMER)
    .executeTakeFirst();
  expect(admitted?.admitted_at ?? null).toBeNull();
});

it("admits a brand-new Google account at a domain no org claimed", async () => {
  const call = await googleSignUpAt("lumen.example");

  const response = await call("/v1/devices", await sessionToken(NEWCOMER));

  expect(response.status).toBe(200);
});
