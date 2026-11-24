import { createHash, createHmac, randomBytes } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import vector from "../../protocol/vectors/share-viewer.json" with { type: "json" };
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import type { Database } from "../src/db.ts";
import { removeDevice } from "../src/devices/removal.ts";
import { bodySha256, requestMessage } from "../src/devices/signature.ts";
import { purgePages } from "../src/pages/store.ts";
import { StashFiles } from "../src/stash/files.ts";
import {
  emptyTables,
  migratedDatabase,
  registered,
  signText,
  type TestDevice,
} from "./accounts.ts";
import { log, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

const ARJUN = "user_arjun";
const DAY = 86_400_000;

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let host: TestDevice;
let sent = 0;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  const stash = new StashFiles({
    dir: mkdtempSync(join(tmpdir(), "sikemux-pages-")),
    key: randomBytes(32),
  });
  app = testApp(database, undefined, { stash });
  host = await registered(app, ARJUN, "host");
});

afterAll(() => drop());

function link() {
  const id = randomBytes(16).toString("base64url");
  const key = randomBytes(32);
  const mac = createHmac("sha256", key)
    .update(`sikemux-share-viewer|v1|${id}`)
    .digest();
  const check = createHash("sha256").update(mac).digest("hex");
  return { id, proof: mac.toString("hex"), check };
}

function asHost(path: string, method: string, init: RequestInit = {}) {
  const body = typeof init.body === "string" ? init.body : "";
  const sha = bodySha256(new TextEncoder().encode(body));
  const time = String(Math.floor(Date.now() / 1000) - (sent++ % 50));
  const message = requestMessage("host", method, path, time, sha);
  return app.request(path, {
    ...init,
    method,
    headers: {
      ...(init.headers as Record<string, string>),
      authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
      ...(method === "PUT" ? { "x-body-sha256": sha } : {}),
    },
  });
}

function share(
  page: ReturnType<typeof link>,
  { anyone = true, expires = Date.now() + DAY, sealed = "c2VhbGVk" } = {},
) {
  return asHost(`/v1/pages/${page.id}`, "PUT", {
    body: sealed,
    headers: {
      "content-type": "application/octet-stream",
      "x-page-viewer-check": page.check,
      "x-page-anyone": String(anyone),
      "x-page-expires": String(expires),
    },
  });
}

function view(id: string, proof: string, token?: string) {
  return app.request(`/v1/pages/${id}`, {
    headers: {
      "x-page-proof": proof,
      ...(token ? { authorization: `Bearer ${token}` } : {}),
    },
  });
}

describe("shared pages", () => {
  it("opens only to the proof the link makes, and counts each view", async () => {
    const page = {
      id: vector.share,
      proof: Buffer.from(vector.proof, "base64url").toString("hex"),
      check: vector.check,
    };
    expect((await share(page)).status).toBe(200);

    const opened = await view(page.id, page.proof);
    expect(opened.status).toBe(200);
    expect(await opened.text()).toBe("c2VhbGVk");

    const wrong = await view(page.id, link().proof);
    expect(wrong.status).toBe(404);
    expect((await view(page.id, page.check)).status).toBe(404);
    expect((await view(page.id, "")).status).toBe(404);

    const state = await asHost(`/v1/pages/${page.id}/state`, "GET");
    expect(await state.json()).toMatchObject({ views: 1, ended: false });
  });

  it("keeps nothing from a host removed while it uploads", async () => {
    const page = link();
    const path = `/v1/pages/${page.id}`;
    const sealed = new TextEncoder().encode("c2VhbGVk");
    const sha = bodySha256(sealed);
    const time = String(Math.floor(Date.now() / 1000) - (sent++ % 50));
    const message = requestMessage("host", "PUT", path, time, sha);
    const response = await app.request(path, {
      method: "PUT",
      body: new ReadableStream(
        {
          async pull(body) {
            await database.db.transaction().execute((trx) =>
              removeDevice(trx, {
                userId: ARJUN,
                key: host.key,
                reason: "removed",
                actor: "system",
              }),
            );
            body.enqueue(sealed);
            body.close();
          },
        },
        { highWaterMark: 0 },
      ),
      duplex: "half",
      headers: {
        authorization: `SikemuxHost ${host.key}.${time}.${signText(host, message)}`,
        "content-type": "application/octet-stream",
        "x-body-sha256": sha,
        "x-page-viewer-check": page.check,
        "x-page-anyone": "true",
        "x-page-expires": String(Date.now() + DAY),
      },
    } as RequestInit);

    expect(response.status).toBe(401);
    expect((await view(page.id, page.proof)).status).toBe(404);
  });

  it("asks a viewer to sign in when the page is not open to anyone", async () => {
    const page = link();
    await share(page, { anyone: false });
    expect((await view(page.id, page.proof)).status).toBe(401);
    const token = await sessionToken("user_viewer");
    expect((await view(page.id, page.proof, token)).status).toBe(200);
  });

  it("answers the same 404 once the share is stopped or expired", async () => {
    const stopped = link();
    await share(stopped);
    expect((await asHost(`/v1/pages/${stopped.id}`, "DELETE")).status).toBe(
      200,
    );
    const afterStop = await view(stopped.id, stopped.proof);

    const expired = link();
    await share(expired);
    await database.db
      .updateTable("pages")
      .set({ expires_at: new Date(Date.now() - 1000) })
      .where("id", "=", expired.id)
      .execute();
    const afterExpiry = await view(expired.id, expired.proof);

    const never = await view(link().id, stopped.proof);
    expect([afterStop.status, afterExpiry.status, never.status]).toEqual([
      404, 404, 404,
    ]);
    const said = async (response: Response) => {
      const { error } = (await response.json()) as {
        error: { code: string; message: string };
      };
      return [error.code, error.message];
    };
    const neverSaid = await said(never);
    expect(await said(afterStop)).toEqual(neverSaid);
    expect(await said(afterExpiry)).toEqual(neverSaid);

    await purgePages(database.db, null, log);
    const row = await database.db
      .selectFrom("pages")
      .select("stored_as")
      .where("id", "=", expired.id)
      .executeTakeFirstOrThrow();
    expect(row.stored_as).toBeNull();
  });

  it("refuses an expiry past 30 days or already gone", async () => {
    expect(
      (await share(link(), { expires: Date.now() + 31 * DAY })).status,
    ).toBe(400);
    expect((await share(link(), { expires: Date.now() - 1 })).status).toBe(400);
  });

  it("lets a second put replace the page under the same link", async () => {
    const page = link();
    await share(page);
    await share(page, { sealed: "bmV3ZXI" });
    expect(await (await view(page.id, page.proof)).text()).toBe("bmV3ZXI");
  });
});
