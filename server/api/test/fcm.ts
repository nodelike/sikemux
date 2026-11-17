import { generateKeyPairSync, verify, type KeyObject } from "node:crypto";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import type { ServiceAccount } from "../src/push/fcm.ts";

export interface FcmReply {
  status: number;
  body?: unknown;
  headers?: Record<string, string>;
}

export interface FakeFcm {
  endpoint: string;
  account: ServiceAccount;
  /** The service account's key file, as Google hands it out. */
  keyFile: Record<string, string>;
  /** Every message FCM accepted or refused, in order. */
  sent: { token: string; body: Record<string, unknown> }[];
  mints: number;
  /** Answers for the next sends, used up in order; after them every send succeeds. */
  replies: FcmReply[];
  nextAccessToken: string;
  stop(): Promise<void>;
}

function readBody(request: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    request.on("error", reject);
  });
}

/** Checks a service account's assertion the way Google does: signature, issuer, audience, scope. */
function assertionValid(
  assertion: string,
  publicKey: KeyObject,
  account: ServiceAccount,
): boolean {
  const [header, claims, signature] = assertion.split(".");
  if (!header || !claims || !signature) return false;
  const ok = verify(
    "sha256",
    Buffer.from(`${header}.${claims}`),
    publicKey,
    Buffer.from(signature, "base64url"),
  );
  const parsed = JSON.parse(Buffer.from(claims, "base64url").toString()) as {
    iss: string;
    aud: string;
    scope: string;
    iat: number;
    exp: number;
  };
  return (
    ok &&
    parsed.iss === account.clientEmail &&
    parsed.aud === account.tokenUri &&
    parsed.scope === "https://www.googleapis.com/auth/firebase.messaging" &&
    parsed.exp - parsed.iat === 3600
  );
}

/** Google's token endpoint and FCM's send endpoint, on one local port. */
export async function fakeFcm(): Promise<FakeFcm> {
  const { publicKey, privateKey } = generateKeyPairSync("rsa", {
    modulusLength: 2048,
  });
  const issued = new Set<string>();
  const fake: FakeFcm = {
    endpoint: "",
    account: undefined as unknown as ServiceAccount,
    keyFile: {},
    sent: [],
    mints: 0,
    replies: [],
    nextAccessToken: "ya29.first",
    stop: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };

  const server: Server = createServer((request, response) => {
    void (async () => {
      const text = await readBody(request);
      const reply = (status: number, body: unknown, headers = {}) => {
        response.writeHead(status, {
          "content-type": "application/json",
          ...headers,
        });
        response.end(JSON.stringify(body));
      };
      if (request.url === "/token") {
        const form = new URLSearchParams(text);
        if (
          form.get("grant_type") !==
            "urn:ietf:params:oauth:grant-type:jwt-bearer" ||
          !assertionValid(form.get("assertion") ?? "", publicKey, fake.account)
        )
          return reply(400, { error: "invalid_grant" });
        fake.mints += 1;
        issued.add(fake.nextAccessToken);
        return reply(200, {
          access_token: fake.nextAccessToken,
          expires_in: 3599,
          token_type: "Bearer",
        });
      }
      const expected = `/v1/projects/${fake.account.projectId}/messages:send`;
      if (request.url !== expected) return reply(404, { error: {} });
      const bearer = /^Bearer (\S+)$/.exec(
        request.headers.authorization ?? "",
      )?.[1];
      if (!bearer || !issued.has(bearer))
        return reply(401, {
          error: { code: 401, status: "UNAUTHENTICATED" },
        });
      const body = JSON.parse(text) as {
        message: { token: string } & Record<string, unknown>;
      };
      fake.sent.push({ token: body.message.token, body: body.message });
      const next = fake.replies.shift();
      if (next) return reply(next.status, next.body ?? {}, next.headers);
      return reply(200, {
        name: `projects/${fake.account.projectId}/messages/${fake.sent.length}`,
      });
    })();
  });
  await new Promise<void>((resolve) =>
    server.listen(0, "127.0.0.1", () => resolve()),
  );
  const { port } = server.address() as AddressInfo;
  fake.endpoint = `http://127.0.0.1:${port}`;
  fake.keyFile = {
    type: "service_account",
    project_id: "sikemux-test",
    private_key_id: "key1",
    private_key: privateKey.export({ format: "pem", type: "pkcs8" }).toString(),
    client_email: "push@sikemux-test.iam.gserviceaccount.com",
    token_uri: `${fake.endpoint}/token`,
  };
  fake.account = {
    projectId: "sikemux-test",
    clientEmail: fake.keyFile.client_email ?? "",
    privateKey,
    privateKeyId: "key1",
    tokenUri: `${fake.endpoint}/token`,
  };
  return fake;
}

/** An FCM error body, as FCM writes them. */
export function fcmError(status: number, code: string): FcmReply {
  return {
    status,
    body: {
      error: {
        code: status,
        status: code,
        details: [
          {
            "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
            errorCode: code,
          },
        ],
      },
    },
  };
}
