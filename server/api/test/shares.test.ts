import { createPrivateKey, verify, createPublicKey } from "node:crypto";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { bodySha256, requestMessage } from "../src/devices/signature.ts";
import { peerMessage, signPeerTicket } from "../src/join/signer.ts";
import vector from "../../protocol/vectors/peer.json" with { type: "json" };
import join from "../../protocol/vectors/join.json" with { type: "json" };
import {
  emptyTables,
  events,
  migratedDatabase,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { teamPlan } from "./billing.ts";
import { FakeClerk } from "./clerk.ts";
import { body, joinSigner, testApp } from "./support.ts";

const ARJUN = "user_arjun";
const MEI = "user_mei";
const OUTSIDER = "user_outsider";
const PROJECT = "ab".repeat(32);

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs cascade");
  await database.pool.query(
    "insert into users (id, staff) select unnest($1::text[]), true",
    [[ARJUN, MEI, OUTSIDER]],
  );
  app = testApp(database, undefined, { clerk: new FakeClerk() });
});

afterAll(() => drop());

let sent = 0;

function signed(
  host: TestDevice,
  path: string,
  method = "GET",
  json?: unknown,
) {
  const text = json === undefined ? "" : JSON.stringify(json);
  const time = String(Math.floor(Date.now() / 1000) - (sent++ % 50));
  const message = requestMessage(
    "host",
    method,
    path,
    time,
    bodySha256(new TextEncoder().encode(text)),
  );
  return app.request(path, {
    method,
    headers: {
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
      "content-type": "application/json",
    },
    ...(json === undefined ? {} : { body: text }),
  });
}

async function org(paid: boolean) {
  const { id } = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: ARJUN })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_users")
    .values([
      { org_id: id, user_id: ARJUN, role: "owner" },
      { org_id: id, user_id: MEI, role: "user" },
    ])
    .execute();
  if (paid) await teamPlan(database.db, id, 5);
  return id;
}

const share = (host: TestDevice, orgId: string, on = host.key) =>
  signed(host, "/v1/account/shares", "PUT", {
    org: orgId,
    project: PROJECT,
    host: on,
  });

const ticket = (host: TestDevice, orgId: string, to: string) =>
  signed(host, "/v1/account/peer-tickets", "POST", { org: orgId, host: to });

describe("the share registry", () => {
  it("shows a teammate's share to the org's other members, never the owner's own", async () => {
    const orgId = await org(true);
    const arjun = await registered(app, ARJUN, "host");
    const mei = await registered(app, MEI, "host");
    expect((await share(arjun, orgId)).status).toBe(204);

    const seen = await body(
      await signed(mei, "/v1/account/shares"),
      "ShareRegistry",
    );
    expect(seen.orgs).toHaveLength(1);
    expect(seen.orgs[0]?.shares).toEqual([
      expect.objectContaining({ project: PROJECT, host: arjun.key }),
    ]);
    expect(seen.orgs[0]?.mine).toEqual([]);
    expect(seen.orgs[0]?.members.map((member) => member.you)).toEqual([
      false,
      true,
    ]);

    const own = await body(
      await signed(arjun, "/v1/account/shares"),
      "ShareRegistry",
    );
    expect(own.orgs[0]?.shares).toEqual([]);
    expect(own.orgs[0]?.mine).toHaveLength(1);
    expect(
      (await events(database))
        .filter((row) => row.type === "shares.changed")
        .map((row) => row.user_id)
        .sort(),
    ).toEqual([ARJUN, MEI]);
  });

  it("refuses a share with an org whose plan lapsed, or on a host of someone else", async () => {
    const lapsed = await org(false);
    const arjun = await registered(app, ARJUN, "host");
    const mei = await registered(app, MEI, "host");
    expect((await share(arjun, lapsed)).status).toBe(404);
    const paid = await org(true);
    expect((await share(arjun, paid, mei.key)).status).toBe(404);
    const seen = await body(
      await signed(mei, "/v1/account/shares"),
      "ShareRegistry",
    );
    expect(seen.orgs.map((one) => one.org)).toEqual([paid]);
  });

  it("forgets a member's shares when they leave the org or unshare", async () => {
    const orgId = await org(true);
    const arjun = await registered(app, ARJUN, "host");
    const mei = await registered(app, MEI, "host");
    await share(arjun, orgId);
    expect(
      (
        await signed(arjun, "/v1/account/shares/remove", "POST", {
          org: orgId,
          project: PROJECT,
        })
      ).status,
    ).toBe(204);
    expect((await ticket(mei, orgId, arjun.key)).status).toBe(404);
    await share(arjun, orgId);
    await database.pool.query("delete from org_users where user_id = $1", [
      ARJUN,
    ]);
    expect((await ticket(mei, orgId, arjun.key)).status).toBe(404);
  });
});

describe("peer tickets", () => {
  it("name the asking member and both hosts, only toward a host that shares with the org", async () => {
    const orgId = await org(true);
    const arjun = await registered(app, ARJUN, "host");
    const mei = await registered(app, MEI, "host");
    const outsider = await registered(app, OUTSIDER, "host");
    expect((await ticket(mei, orgId, arjun.key)).status).toBe(404);
    await share(arjun, orgId);

    const issued = await body(
      await ticket(mei, orgId, arjun.key),
      "PeerTicket",
    );
    expect(issued).toMatchObject({ org: orgId, from: mei.key, to: arjun.key });
    const { signature, ...unsigned } = issued;
    expect(
      verify(
        null,
        Buffer.from(peerMessage(unsigned), "utf8"),
        createPublicKey(joinSigner.privateKey),
        Buffer.from(signature, "hex"),
      ),
    ).toBe(true);

    expect((await ticket(outsider, orgId, arjun.key)).status).toBe(404);
    expect((await ticket(arjun, orgId, arjun.key)).status).toBe(404);
  });

  it("sign the text in the vector the core checks", () => {
    const privateKey = createPrivateKey(join.privateKeyPem);
    const { ticket: expected } = vector;
    const made = signPeerTicket(
      { keyId: expected.keyId, privateKey },
      {
        org: expected.org,
        member: expected.member,
        memberName: expected.memberName,
        from: expected.from,
        to: expected.to,
      },
      expected.issuedAt * 1000,
    );
    expect(made).toEqual(expected);
    const { signature: _signature, ...unsigned } = made;
    expect(peerMessage(unsigned)).toBe(vector.message);
  });
});
