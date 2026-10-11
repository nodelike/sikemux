import { generateKeyPairSync, verify } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  appleSignIn,
  clientSecret,
  readAppleSignIn,
} from "../src/account/apple.ts";
import type { AppleKey } from "../src/apple-key.ts";

const { publicKey, privateKey } = generateKeyPairSync("ec", {
  namedCurve: "prime256v1",
});
const key: AppleKey = { keyId: "75DSC62NQZ", teamId: "D577WD6Z5U", privateKey };
const PHONE = "com.nodelike.sikemux.mobile";
const NOW = 1_790_000_000_000;

function decode(token: string) {
  const [header, claims, signature] = token.split(".");
  return {
    header: JSON.parse(Buffer.from(header ?? "", "base64url").toString()),
    claims: JSON.parse(Buffer.from(claims ?? "", "base64url").toString()),
    verified: verify(
      "sha256",
      Buffer.from(`${header}.${claims}`),
      { key: publicKey, dsaEncoding: "ieee-p1363" },
      Buffer.from(signature ?? "", "base64url"),
    ),
  };
}

interface Posted {
  url: string;
  form: Record<string, string>;
}

/** Apple's endpoints as a stub: each path answers with the next reply queued for it, or 200. */
function stubApple(
  replies: Record<string, { status: number; body?: object }[]> = {},
) {
  const posted: Posted[] = [];
  const fetcher = async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    posted.push({
      url,
      form: Object.fromEntries(new URLSearchParams(String(init?.body))),
    });
    const path = new URL(url).pathname;
    const reply = replies[path]?.shift() ?? {
      status: 200,
      body:
        path === "/auth/token"
          ? { access_token: "at", refresh_token: "rt", id_token: "id" }
          : undefined,
    };
    return new Response(reply.body ? JSON.stringify(reply.body) : null, {
      status: reply.status,
    });
  };
  return { posted, apple: appleSignIn(key, { fetcher, now: () => NOW }) };
}

describe("Sign in with Apple's client secret", () => {
  it("is an ES256 token for the client, signed with the team's key", () => {
    const { header, claims, verified } = decode(clientSecret(key, PHONE, NOW));
    expect(verified).toBe(true);
    expect(header).toEqual({ alg: "ES256", kid: "75DSC62NQZ" });
    expect(claims).toEqual({
      iss: "D577WD6Z5U",
      iat: NOW / 1000,
      exp: NOW / 1000 + 300,
      aud: "https://appleid.apple.com",
      sub: PHONE,
    });
  });
});

describe("revoking Sign in with Apple", () => {
  it("trades the phone's code for a refresh token, then revokes it", async () => {
    const { posted, apple } = stubApple();
    await apple.revokeCode("c0de", PHONE);
    expect(posted.map(({ url }) => url)).toEqual([
      "https://appleid.apple.com/auth/token",
      "https://appleid.apple.com/auth/revoke",
    ]);
    expect(posted[0]?.form).toEqual({
      client_id: PHONE,
      client_secret: expect.any(String),
      code: "c0de",
      grant_type: "authorization_code",
    });
    expect(posted[1]?.form).toEqual({
      client_id: PHONE,
      client_secret: expect.any(String),
      token: "rt",
      token_type_hint: "refresh_token",
    });
    for (const { form } of posted) {
      const { claims, verified } = decode(form.client_secret ?? "");
      expect(verified).toBe(true);
      expect(claims.sub).toBe(PHONE);
    }
  });

  it("revokes an access token Clerk held", async () => {
    const { posted, apple } = stubApple();
    await apple.revokeAccessToken("at", "com.nodelike.sikemux.signin");
    expect(posted).toEqual([
      {
        url: "https://appleid.apple.com/auth/revoke",
        form: {
          client_id: "com.nodelike.sikemux.signin",
          client_secret: expect.any(String),
          token: "at",
          token_type_hint: "access_token",
        },
      },
    ]);
  });

  it("fails with Apple's error code, never the code or token it sent", async () => {
    const { posted, apple } = stubApple({
      "/auth/token": [{ status: 400, body: { error: "invalid_grant" } }],
    });
    const failure = apple.revokeCode("secret-code", PHONE);
    await expect(failure).rejects.toThrow(
      "Apple answered 400 to /auth/token (invalid_grant)",
    );
    await expect(failure).rejects.not.toThrow(/secret-code/);
    expect(posted).toHaveLength(1);
  });

  it("fails when the revocation is refused", async () => {
    const { apple } = stubApple({
      "/auth/revoke": [{ status: 400, body: { error: "invalid_client" } }],
    });
    await expect(apple.revokeCode("c0de", PHONE)).rejects.toThrow(
      "Apple answered 400 to /auth/revoke (invalid_client)",
    );
  });
});

describe("Sign in with Apple settings", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "sikemux-apple-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  it("read the key with its ids", () => {
    const path = join(dir, "signin.p8");
    writeFileSync(
      path,
      privateKey.export({ format: "pem", type: "pkcs8" }).toString(),
    );
    const problems: string[] = [];
    expect(
      readAppleSignIn(
        {
          APPLE_SIGNIN_KEY_FILE: path,
          APPLE_SIGNIN_KEY_ID: "75DSC62NQZ",
          APPLE_SIGNIN_TEAM_ID: "D577WD6Z5U",
        },
        problems,
      ),
    ).toMatchObject({ keyId: "75DSC62NQZ", teamId: "D577WD6Z5U" });
    expect(problems).toEqual([]);
  });

  it("name what is wrong", () => {
    const problems: string[] = [];
    readAppleSignIn(
      { APPLE_SIGNIN_KEY_FILE: join(dir, "missing.p8") },
      problems,
    );
    expect(problems).toEqual([
      "APPLE_SIGNIN_KEY_ID is not a key id like ABC123DEFG",
      "APPLE_SIGNIN_TEAM_ID is not a team id like D577WD6Z5U",
      `APPLE_SIGNIN_KEY_FILE ${join(dir, "missing.p8")} cannot be read (ENOENT)`,
    ]);
  });
});
