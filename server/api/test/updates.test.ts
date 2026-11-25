import { createHash, verify, X509Certificate } from "node:crypto";
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { openDatabase, type Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { migrate, readMigrations } from "../src/migrations.ts";
import {
  assetFileName,
  promoteUpdate,
  publishUpdate,
  withdrawUpdate,
} from "../src/updates/publish.ts";
import { body, freshDatabase, log, testApp } from "./support.ts";
import {
  readMultipart,
  signingKey,
  writeBundle,
  type BundleOptions,
} from "./updates.ts";

let database: Database;
let drop: () => Promise<void>;
let assetsDir: string;
const { key, certificate } = signingKey();
const stranger = signingKey();

beforeAll(async () => {
  const fresh = await freshDatabase();
  drop = fresh.drop;
  database = openDatabase(fresh.url, log);
  await migrate(
    database.pool,
    await readMigrations(new URL("../migrations", import.meta.url).pathname),
    log,
  );
});

beforeEach(async () => {
  await database.pool.query("truncate updates, update_channels, audit cascade");
  assetsDir = mkdtempSync(join(tmpdir(), "sikemux-update-assets-"));
});

afterAll(async () => {
  await database.close();
  await drop();
});

function bundle(options: Omit<BundleOptions, "key"> = {}) {
  return writeBundle({ key, ...options });
}

function publish(dir: string) {
  return publishUpdate(database.db, { dir, assetsDir, certificate });
}

const HEADERS = {
  "expo-protocol-version": "1",
  "expo-platform": "android",
  "expo-runtime-version": "runtime-1",
  "expo-channel-name": "nightly",
  "expo-expect-signature": 'sig, keyid="main", alg="rsa-v1_5-sha256"',
};

function askForUpdate(headers: Record<string, string> = {}) {
  return testApp(database).request("/updates/manifest", {
    headers: { ...HEADERS, ...headers },
  });
}

async function servedManifest(response: Response) {
  expect(response.status).toBe(200);
  const parts = readMultipart(
    response.headers.get("content-type") ?? "",
    Buffer.from(await response.arrayBuffer()),
  );
  expect(parts).toHaveLength(1);
  const [part] = parts;
  if (!part) throw new Error("no part");
  expect(part.headers["content-disposition"]).toBe(
    'form-data; name="manifest"',
  );
  expect(part.headers["content-type"]).toMatch(/^application\/json/);
  return part;
}

async function servedId(headers: Record<string, string> = {}) {
  const part = await servedManifest(await askForUpdate(headers));
  return JSON.parse(part.body.toString()).id;
}

async function auditActions() {
  const { rows } = await database.pool.query(
    "select action from audit order by id",
  );
  return rows.map((row) => row.action);
}

describe("GET /updates/manifest", () => {
  it("serves the stored manifest byte for byte, with the stored signature", async () => {
    const published = bundle();
    await publish(published.dir);

    const response = await askForUpdate();
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toMatch(
      /^multipart\/mixed; boundary=/,
    );
    expect(response.headers.get("expo-protocol-version")).toBe("1");
    expect(response.headers.get("expo-sfv-version")).toBe("0");
    expect(response.headers.get("cache-control")).toBe("private, max-age=0");

    const part = await servedManifest(response);
    expect(part.body.equals(published.manifest)).toBe(true);
    expect(part.headers["expo-signature"]).toBe(
      `sig="${published.signature}", keyid="main", alg="rsa-v1_5-sha256"`,
    );
    const signature = /sig="([^"]+)"/.exec(
      part.headers["expo-signature"] ?? "",
    )?.[1];
    expect(
      verify(
        "sha256",
        part.body,
        new X509Certificate(certificate).publicKey,
        Buffer.from(signature ?? "", "base64"),
      ),
    ).toBe(true);
  });

  it("offers the newest update for the phone's platform, runtime and channel", async () => {
    await publish(bundle({ createdAt: "2026-10-01T00:00:00.000Z" }).dir);
    const newest = bundle({ createdAt: "2026-10-02T00:00:00.000Z" });
    await publish(newest.dir);
    await publish(
      bundle({ createdAt: "2026-10-03T00:00:00.000Z", platform: "ios" }).dir,
    );
    await publish(
      bundle({
        createdAt: "2026-10-03T00:00:00.000Z",
        runtimeVersion: "runtime-2",
      }).dir,
    );
    await publish(
      bundle({ createdAt: "2026-10-03T00:00:00.000Z", channel: "stable" }).dir,
    );

    const part = await servedManifest(await askForUpdate());
    expect(JSON.parse(part.body.toString()).id).toBe(newest.id);
  });

  it("answers an empty 204 when there is nothing to offer, which phones take as no update", async () => {
    await publish(bundle({ channel: "stable" }).dir);
    const response = await askForUpdate();
    expect(response.status).toBe(204);
    expect(response.headers.get("expo-protocol-version")).toBe("1");
    expect(response.headers.get("content-type")).toBeNull();
    expect(await response.text()).toBe("");
  });

  it.each([
    ["expo-protocol-version", "0"],
    ["expo-protocol-version", ""],
    ["expo-platform", "macos"],
    ["expo-platform", ""],
    ["expo-runtime-version", ""],
    ["expo-runtime-version", "has spaces"],
    ["expo-channel-name", "dev"],
    ["expo-channel-name", ""],
  ])("refuses %s %j", async (name, value) => {
    const response = await askForUpdate({ [name]: value });
    expect(response.status).toBe(400);
    expect((await body(response, "ApiError")).error.code).toBe("bad_request");
  });

  it("limits how often one address asks", async () => {
    const app = testApp(database, new RateLimiter());
    const statuses = [];
    for (let i = 0; i < 121; i++) {
      const response = await app.request("/updates/manifest", {
        headers: { ...HEADERS, "x-forwarded-for": "198.51.100.7" },
      });
      statuses.push(response.status);
    }
    expect(statuses.slice(0, 120).every((status) => status === 204)).toBe(true);
    expect(statuses[120]).toBe(429);
  });
});

describe("publishUpdate", () => {
  it("stores each asset under its hash and media type, and records the update once", async () => {
    const published = bundle();
    const first = await publish(published.dir);
    expect(first).toMatchObject({
      id: published.id,
      added: true,
      assetsAdded: 2,
    });

    const [launch, svg] = published.hexes as [string, string];
    expect(readdirSync(assetsDir).sort()).toEqual(
      [
        assetFileName(launch, "application/javascript"),
        assetFileName(svg, "image/svg+xml"),
      ].sort(),
    );
    expect(assetFileName(svg, "image/svg+xml")).toBe(`${svg}.image.svg+xml`);
    const stored = readFileSync(join(assetsDir, `${svg}.image.svg+xml`));
    expect(createHash("sha256").update(stored).digest("hex")).toBe(svg);

    const again = await publish(published.dir);
    expect(again).toMatchObject({ added: false, assetsAdded: 0 });
    const { rows } = await database.pool.query(
      "select action, subject from audit",
    );
    expect(rows).toEqual([
      { action: "update.published", subject: published.id },
    ]);
  });

  it("never replaces an asset that is already stored", async () => {
    const published = bundle();
    const [launch] = published.hexes as [string];
    const name = assetFileName(launch, "application/javascript");
    writeFileSync(join(assetsDir, name), "kept");
    await publish(published.dir);
    expect(readFileSync(join(assetsDir, name), "utf8")).toBe("kept");
  });

  it("refuses an asset whose bytes do not match its hash", async () => {
    const published = bundle();
    writeFileSync(
      join(published.dir, "assets", published.hexes[0] ?? ""),
      "tampered",
    );
    await expect(publish(published.dir)).rejects.toThrow(
      "does not match its SHA-256",
    );
    expect(readdirSync(assetsDir)).toEqual([]);
  });

  it("refuses a manifest the update key did not sign", async () => {
    const published = writeBundle({ key: stranger.key });
    await expect(publish(published.dir)).rejects.toThrow(
      "is not a signature of manifest.json",
    );
    expect(readdirSync(assetsDir)).toEqual([]);
  });

  it("refuses a manifest changed after it was signed", async () => {
    const published = bundle();
    writeFileSync(
      join(published.dir, "manifest.json"),
      published.manifest.toString().replace("{}", '{"x":1}'),
    );
    await expect(publish(published.dir)).rejects.toThrow(
      "is not a signature of manifest.json",
    );
  });

  it("refuses a missing asset", async () => {
    const published = bundle();
    rmSync(join(published.dir, "assets", published.hexes[1] ?? ""));
    await expect(publish(published.dir)).rejects.toThrow("has no file");
  });

  it("refuses an asset that is a link to somewhere else", async () => {
    const published = bundle();
    const path = join(published.dir, "assets", published.hexes[1] ?? "");
    rmSync(path);
    symlinkSync("/etc/hosts", path);
    await expect(publish(published.dir)).rejects.toThrow("has no file");
  });

  it("refuses files the manifest does not name", async () => {
    const published = bundle();
    writeFileSync(join(published.dir, "assets", "..secret"), "x");
    await expect(publish(published.dir)).rejects.toThrow(
      "assets/..secret is not named by the manifest",
    );
    rmSync(join(published.dir, "assets", "..secret"));
    writeFileSync(join(published.dir, "extra"), "x");
    await expect(publish(published.dir)).rejects.toThrow(
      "it must hold exactly",
    );
  });

  it("refuses asset addresses that point anywhere but the asset folder", async () => {
    const published = bundle();
    const manifest = published.manifest
      .toString()
      .replace(
        `assets/${published.hexes[1]}`,
        `assets/../${published.hexes[1]}`,
      );
    writeFileSync(join(published.dir, "manifest.json"), manifest);
    await expect(publish(published.dir)).rejects.toThrow(
      "does not end in a SHA-256",
    );
  });

  it("refuses a content type that disagrees with the manifest", async () => {
    const published = bundle();
    writeFileSync(
      join(published.dir, "assets", `${published.hexes[1]}.type`),
      "text/html\n",
    );
    await expect(publish(published.dir)).rejects.toThrow(
      "but the manifest says image/svg+xml",
    );
  });

  it("refuses a second update with an id already taken", async () => {
    const first = bundle();
    await publish(first.dir);
    const second = bundle({ id: first.id, runtimeVersion: "runtime-2" });
    await expect(publish(second.dir)).rejects.toThrow(
      "already published with other contents",
    );
  });

  it("refuses a malformed update.json", async () => {
    const published = bundle({ platform: "macos" });
    await expect(publish(published.dir)).rejects.toThrow(
      "platform is not android or ios",
    );
  });
});

describe("promoteUpdate", () => {
  it("offers a nightly update on stable with the same signed manifest", async () => {
    const published = bundle();
    await publish(published.dir);
    expect((await askForUpdate({ "expo-channel-name": "stable" })).status).toBe(
      204,
    );

    const promoted = await promoteUpdate(database.db, published.id);
    expect(promoted).toMatchObject({ id: published.id, added: true });
    const part = await servedManifest(
      await askForUpdate({ "expo-channel-name": "stable" }),
    );
    expect(part.body.equals(published.manifest)).toBe(true);

    expect(await promoteUpdate(database.db, published.id)).toMatchObject({
      added: false,
    });
    const { rows } = await database.pool.query(
      "select action from audit order by id",
    );
    expect(rows.map((row) => row.action)).toEqual([
      "update.published",
      "update.promoted",
    ]);
  });

  it("refuses an id no update has", async () => {
    await expect(
      promoteUpdate(database.db, "4b0b9a5e-1c6f-4a8e-9d3a-2f6b7c8d9e0f"),
    ).rejects.toThrow("No update has the id");
    await expect(promoteUpdate(database.db, "not-an-id")).rejects.toThrow(
      "is not an update id",
    );
  });

  it("refuses an update older than what stable already offers", async () => {
    const older = bundle({ createdAt: "2026-10-01T00:00:00.000Z" });
    const newer = bundle({
      createdAt: "2026-10-02T00:00:00.000Z",
      channel: "stable",
    });
    await publish(older.dir);
    await publish(newer.dir);
    await expect(promoteUpdate(database.db, older.id)).rejects.toThrow(
      "phones would never take it",
    );
  });
});

describe("withdrawUpdate", () => {
  it("stops offering an update on every channel, falling back to the one before it", async () => {
    const older = bundle({ createdAt: "2026-10-01T00:00:00.000Z" });
    const newer = bundle({ createdAt: "2026-10-02T00:00:00.000Z" });
    await publish(older.dir);
    await publish(newer.dir);
    await promoteUpdate(database.db, newer.id);
    expect(await servedId({ "expo-channel-name": "stable" })).toBe(newer.id);

    expect(await withdrawUpdate(database.db, newer.id)).toMatchObject({
      id: newer.id,
      channels: expect.arrayContaining(["nightly", "stable"]),
    });
    expect(await servedId()).toBe(older.id);
    expect((await askForUpdate({ "expo-channel-name": "stable" })).status).toBe(
      204,
    );

    expect(await withdrawUpdate(database.db, newer.id)).toMatchObject({
      channels: [],
    });
    expect(await auditActions()).toEqual([
      "update.published",
      "update.published",
      "update.promoted",
      "update.withdrawn",
    ]);
  });

  it("keeps a withdrawn update withdrawn when it is published or promoted again", async () => {
    const published = bundle();
    await publish(published.dir);
    await withdrawUpdate(database.db, published.id);
    await expect(publish(published.dir)).rejects.toThrow(
      `${published.id} was withdrawn`,
    );
    await expect(promoteUpdate(database.db, published.id)).rejects.toThrow(
      `${published.id} was withdrawn`,
    );
    expect((await askForUpdate()).status).toBe(204);
  });

  it("refuses an id no update has", async () => {
    await expect(
      withdrawUpdate(database.db, "4b0b9a5e-1c6f-4a8e-9d3a-2f6b7c8d9e0f"),
    ).rejects.toThrow("No update has the id");
    await expect(withdrawUpdate(database.db, "not-an-id")).rejects.toThrow(
      "is not an update id",
    );
  });

  it("lets stable offer an older update once the newer one is withdrawn, serving whatever was put there last", async () => {
    const older = bundle({ createdAt: "2026-10-01T00:00:00.000Z" });
    const newer = bundle({
      createdAt: "2026-10-02T00:00:00.000Z",
      channel: "stable",
    });
    await publish(older.dir);
    await publish(newer.dir);
    await expect(promoteUpdate(database.db, older.id)).rejects.toThrow(
      `Withdraw ${newer.id} first`,
    );

    await withdrawUpdate(database.db, newer.id);
    await promoteUpdate(database.db, older.id);
    expect(await servedId({ "expo-channel-name": "stable" })).toBe(older.id);
  });

  it("refuses to publish an update older than one the channel still offers", async () => {
    const newer = bundle({ createdAt: "2026-10-02T00:00:00.000Z" });
    const older = bundle({ createdAt: "2026-10-01T00:00:00.000Z" });
    await publish(newer.dir);
    await expect(publish(older.dir)).rejects.toThrow(
      "phones would never take it",
    );
    await withdrawUpdate(database.db, newer.id);
    await publish(older.dir);
    expect(await servedId()).toBe(older.id);
  });
});
