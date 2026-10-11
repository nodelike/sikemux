import { generateKeyPairSync } from "node:crypto";
import { mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import joinVector from "../../protocol/vectors/join.json" with { type: "json" };
import { loadConfig, loadMigrationConfig } from "../src/config.ts";
import { devKeyFile } from "../src/join/signer.ts";
import { readNetwork } from "../src/network/network.ts";
import { joinKeyFile } from "./support.ts";

const keyFile = joinKeyFile();
const minimal = {
  DATABASE_URL: "postgresql://sikemux@localhost/sikemux",
  CLERK_ISSUER: "https://clerk.sikemux.com",
  CLERK_MAC_CLIENT_ID: "mac_client",
  JOIN_SIGNING_KEY_FILE: keyFile,
};

describe("loadConfig", () => {
  it("listens only on this machine unless told otherwise", () => {
    expect(loadConfig(minimal)).toEqual({
      host: "127.0.0.1",
      port: 4000,
      databaseUrl: minimal.DATABASE_URL,
      appOrigin: "https://app.sikemux.com",
      clerkIssuer: "https://clerk.sikemux.com",
      macClientId: "mac_client",
      clerkSecretKey: null,
      clerkWebhookSecret: null,
      network: readNetwork({}, []),
      push: { app: "production", allowSandbox: false, fcm: null, apns: null },
      appleSignIn: null,
      join: {
        keyId: "dev-1",
        privateKey: expect.anything(),
        publicKey: joinVector.publicKey,
        file: keyFile,
        created: false,
      },
      logLevel: "info",
    });
  });

  it("refuses secrets that are not Clerk's", () => {
    expect(() =>
      loadConfig({
        ...minimal,
        CLERK_SECRET_KEY: "pk_live_abc",
        CLERK_WEBHOOK_SECRET: "c2VjcmV0",
      }),
    ).toThrow(
      "CLERK_SECRET_KEY is not a secret key like sk_live_…; CLERK_WEBHOOK_SECRET is not a signing secret like whsec_…",
    );
  });

  it("lists every problem at once", () => {
    expect(() =>
      loadConfig({
        PORT: "eighty",
        APP_ORIGIN: "https://app.sikemux.com/",
        CLERK_ISSUER: "http://clerk.sikemux.com",
        LOG_LEVEL: "loud",
        JOIN_SIGNING_KEY_FILE: keyFile,
      }),
    ).toThrow(
      "The API cannot start: DATABASE_URL is not set; PORT is not a port number; APP_ORIGIN is not an origin like https://app.sikemux.com; CLERK_ISSUER is not an https origin like https://clerk.sikemux.com; CLERK_MAC_CLIENT_ID is not set; LOG_LEVEL is not one of fatal, error, warn, info, debug, trace.",
    );
  });
});

describe("the join signing key", () => {
  const { JOIN_SIGNING_KEY_FILE: _file, ...withoutKey } = minimal;

  it("is prod-1 from JOIN_SIGNING_KEY_FILE in a release build", () => {
    expect(loadConfig(minimal, { release: true }).join).toMatchObject({
      keyId: "prod-1",
      publicKey: joinVector.publicKey,
      file: keyFile,
    });
  });

  it("is required in a release build, which never makes its own", () => {
    const home = mkdtempSync(join(tmpdir(), "sikemux-home-"));
    expect(() =>
      loadConfig({ ...withoutKey, HOME: home }, { release: true }),
    ).toThrow("The API cannot start: JOIN_SIGNING_KEY_FILE is not set.");
    expect(() => statSync(devKeyFile(home))).toThrow();
  });

  it("is made once in the home folder for a dev API, readable only by its owner", () => {
    const home = mkdtempSync(join(tmpdir(), "sikemux-home-"));
    const first = loadConfig({ ...withoutKey, HOME: home }).join;
    expect(first).toMatchObject({
      keyId: "dev-1",
      file: join(home, ".config/sikemux/dev/join-signing-key.pem"),
      created: true,
    });
    expect(statSync(first.file).mode & 0o777).toBe(0o600);
    expect(readFileSync(first.file, "utf8")).toContain(
      "-----BEGIN PRIVATE KEY-----",
    );
    const again = loadConfig({ ...withoutKey, HOME: home }).join;
    expect(again).toMatchObject({
      publicKey: first.publicKey,
      created: false,
    });
  });

  it("refuses a missing file, a key that is not Ed25519 and an id that would break the ticket", () => {
    const rsa = join(mkdtempSync(join(tmpdir(), "sikemux-join-")), "rsa.pem");
    writeFileSync(
      rsa,
      generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey.export({
        format: "pem",
        type: "pkcs8",
      }),
    );
    expect(() =>
      loadConfig({ ...minimal, JOIN_SIGNING_KEY_FILE: "/nonexistent/key.pem" }),
    ).toThrow("JOIN_SIGNING_KEY_FILE /nonexistent/key.pem cannot be used");
    expect(() =>
      loadConfig({ ...minimal, JOIN_SIGNING_KEY_FILE: rsa }),
    ).toThrow(
      `JOIN_SIGNING_KEY_FILE ${rsa} cannot be used: is not an Ed25519 private key`,
    );
    expect(() =>
      loadConfig({ ...minimal, JOIN_SIGNING_KEY_ID: "prod|1" }),
    ).toThrow("JOIN_SIGNING_KEY_ID is not lowercase words joined by dashes");
  });
});

describe("loadMigrationConfig", () => {
  it("needs only the database, so a deploy migrates without the server's settings", () => {
    expect(loadMigrationConfig({ DATABASE_URL: minimal.DATABASE_URL })).toEqual(
      { databaseUrl: minimal.DATABASE_URL, logLevel: "info" },
    );
  });

  it("still refuses a database that is not postgres", () => {
    expect(() =>
      loadMigrationConfig({ DATABASE_URL: "mysql://localhost/sikemux" }),
    ).toThrow("DATABASE_URL is not a postgres:// URL");
  });
});
