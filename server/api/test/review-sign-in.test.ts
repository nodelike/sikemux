import { afterAll, beforeEach, describe, expect, it } from "vitest";

import { ReviewAccount } from "../src/account/review-sign-in.ts";
import { RateLimiter } from "../src/limits.ts";
import { FakeClerk } from "./clerk.ts";
import { log, testApp, unreachableDatabase } from "./support.ts";

const database = unreachableDatabase();
afterAll(() => database.close());

const DEMO = "review@acme.example";
const PASSWORD = "the demo account's password";

let clerk: FakeClerk;
let now: number;
let app: ReturnType<typeof testApp>;

beforeEach(async () => {
  clerk = new FakeClerk();
  clerk.demos.set("user_review", {
    address: DEMO,
    addresses: [DEMO],
    passwordSetAt: 1,
    secondFactor: false,
  });
  clerk.passwords.set("user_review", PASSWORD);
  clerk.passwords.set("user_maya", PASSWORD);
  now = Date.parse("2026-11-20T09:00:00Z");
  const review = new ReviewAccount("user_review", {
    now: () => now,
    answerAfterMs: 0,
  });
  await review.load(clerk, log);
  app = testApp(database, new RateLimiter(() => now), { clerk, review });
});

const signIn = async (email: string, password: string, from = "192.0.2.1") => {
  const response = await app.request("/v1/review-sign-in", {
    method: "POST",
    headers: { "content-type": "application/json", "x-forwarded-for": from },
    body: JSON.stringify({ email, password }),
  });
  const body = (await response.json()) as {
    ticket?: string;
    error?: { requestId?: string };
  };
  delete body.error?.requestId;
  return { status: response.status, body };
};

describe("POST /v1/review-sign-in", () => {
  it("refuses a wrong password and makes no sign-in token", async () => {
    const refused = await signIn(DEMO, "not the password");
    expect(refused.status).toBe(401);
    expect(refused.body.ticket).toBeUndefined();
    expect(clerk.signInTokens).toEqual([]);

    const allowed = await signIn(` Review@Acme.example `, PASSWORD);
    expect(allowed).toEqual({
      status: 200,
      body: { ticket: "sign-in token 0 for user_review" },
    });
  });

  it("refuses any other address the same way, even with that account's own password", async () => {
    const wrongPassword = await signIn(DEMO, "not the password");
    const otherAddress = await signIn("maya@acme.example", PASSWORD);
    expect(otherAddress).toEqual(wrongPassword);
    expect(clerk.signInTokens).toEqual([]);
  });

  it("limits each caller to five tries a minute", async () => {
    const tries = [];
    for (let i = 0; i < 6; i++)
      tries.push((await signIn(DEMO, PASSWORD, "203.0.113.1")).status);
    expect(tries).toEqual([200, 200, 200, 200, 200, 429]);
    expect((await signIn(DEMO, PASSWORD, "203.0.113.2")).status).toBe(200);
  });
});
