import { schema } from "@sikemux/protocol";
import { afterAll, describe, expect, it } from "vitest";

import { loadConfig } from "../src/config.ts";
import { RateLimiter } from "../src/limits.ts";
import { readNetwork } from "../src/network/network.ts";
import { body, joinKeyFile, testApp, unreachableDatabase } from "./support.ts";

const database = unreachableDatabase();
afterAll(() => database.close());

describe("GET /v1/network", () => {
  it("names our relay and allows every version until told otherwise, without signing in", async () => {
    const response = await testApp(database).request("/v1/network");
    expect(response.status).toBe(200);
    expect(await body(response, "Network")).toEqual({
      relays: [
        {
          url: "https://relay.sikemux.com/",
          region: "mumbai",
          quicPort: 7842,
        },
      ],
      minimumVersions: {
        macos: { nightly: "0.0.0", stable: "0.0.0" },
        linux: { nightly: "0.0.0", stable: "0.0.0" },
        ios: { nightly: "0.0.0", stable: "0.0.0" },
        android: { nightly: "0.0.0", stable: "0.0.0" },
      },
    });
  });

  it("lets caches keep it for five minutes and answers 304 to a matching ETag", async () => {
    const app = testApp(database);
    const first = await app.request("/v1/network");
    expect(first.headers.get("cache-control")).toBe("public, max-age=300");
    const tag = first.headers.get("etag");
    expect(tag).toBeTruthy();
    const again = await app.request("/v1/network", {
      headers: { "if-none-match": tag ?? "" },
    });
    expect(again.status).toBe(304);
  });

  it("answers what the environment sets", async () => {
    const network = readNetwork(
      {
        RELAY_URL: "https://relay.example.com",
        RELAY_REGION: "eu",
        RELAY_QUIC_PORT: "none",
        MINIMUM_VERSION_MACOS_STABLE: "0.5.0",
        MINIMUM_VERSION_IOS_NIGHTLY: "0.2.0-nightly.4",
      },
      [],
    );
    const response = await testApp(database, new RateLimiter(), {
      network,
    }).request("/v1/network");
    expect(await body(response, "Network")).toEqual({
      relays: [
        { url: "https://relay.example.com/", region: "eu", quicPort: null },
      ],
      minimumVersions: {
        macos: { nightly: "0.0.0", stable: "0.5.0" },
        linux: { nightly: "0.0.0", stable: "0.0.0" },
        ios: { nightly: "0.2.0-nightly.4", stable: "0.0.0" },
        android: { nightly: "0.0.0", stable: "0.0.0" },
      },
    });
  });

  it("allows 60 requests a minute from one address", async () => {
    const app = testApp(database);
    const headers = { "x-forwarded-for": "203.0.113.9" };
    for (let i = 0; i < 60; i++)
      expect((await app.request("/v1/network", { headers })).status).toBe(200);
    const refused = await app.request("/v1/network", { headers });
    expect(refused.status).toBe(429);
    expect(await body(refused, "ApiError")).toMatchObject({
      error: { code: "rate_limited" },
    });
  });
});

describe("readNetwork", () => {
  it("covers every platform devices register with", () => {
    const platforms = schema.$defs.Platform.enum;
    expect(Object.keys(readNetwork({}, []).minimumVersions).sort()).toEqual(
      [...platforms].sort(),
    );
  });

  it("refuses a relay or version the apps could not use, naming each", () => {
    expect(() =>
      loadConfig({
        DATABASE_URL: "postgresql://sikemux@localhost/sikemux",
        CLERK_ISSUER: "https://clerk.sikemux.com",
        CLERK_MAC_CLIENT_ID: "mac_client",
        JOIN_SIGNING_KEY_FILE: joinKeyFile(),
        RELAY_URL: "http://relay.sikemux.com",
        RELAY_QUIC_PORT: "70000",
        MINIMUM_VERSION_ANDROID_STABLE: "1.2",
      }),
    ).toThrow(
      'The API cannot start: RELAY_URL is not an https address like https://relay.sikemux.com/; RELAY_QUIC_PORT is not a port number or "none"; MINIMUM_VERSION_ANDROID_STABLE is not a version like 0.5.0 or 0.6.0-nightly.3.',
    );
  });
});
