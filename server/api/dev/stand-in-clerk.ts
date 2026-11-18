import { createServer } from "node:http";
import { exportJWK, generateKeyPair, SignJWT } from "jose";

/**
 * A sign-in for made-up people on a local run, so the signed-in app can be seen without a real
 * account. Point the local API's CLERK_ISSUER and the app's VITE_STAND_IN_CLERK at it. It also
 * answers the Backend API's user reads and deletes, so the API can name them and fold them away.
 * Maya and Kofi each have two accounts: a personal one, and one made with a work address at
 * acme.example, Maya's just now. Maya also has a third, made just now with another address of
 * her own, for the catch that only asks.
 */
const LONG_AGO = Date.parse("2025-06-01T09:00:00Z");
const STARTED = Date.now();

const PEOPLE: {
  key: string;
  id: string;
  name: string;
  domain: string;
  address?: string;
  createdAt?: number;
}[] = [
  {
    key: "ines",
    id: "user_standinines",
    name: "Inês Duarte",
    domain: "lumen.test",
  },
  {
    key: "tomas",
    id: "user_standintomas",
    name: "Tomás Reyes",
    domain: "lumen.test",
  },
  {
    key: "priya",
    id: "user_standinpriya",
    name: "Priya Natarajan",
    domain: "lumen.test",
  },
  {
    key: "jonah",
    id: "user_standinjonah",
    name: "Jonah Lindqvist",
    domain: "fjord.test",
    alsoAt: ["lumen.test"],
  },
  {
    key: "amara",
    id: "user_standinamara",
    name: "Amara Okafor",
    domain: "lumen.test",
  },
  {
    key: "maya",
    id: "user_standinmaya",
    name: "Maya Okafor",
    domain: "okafor.example",
  },
  {
    key: "mayawork",
    id: "user_standinmayawork",
    name: "Maya Okafor",
    domain: "acme.example",
    address: "maya@acme.example",
    createdAt: STARTED,
  },
  {
    key: "kofi",
    id: "user_standinkofi",
    name: "Kofi Mensah",
    domain: "mensah.example",
  },
  {
    key: "kofiwork",
    id: "user_standinkofiwork",
    name: "Kofi Mensah",
    domain: "acme.example",
    address: "kofi@acme.example",
  },
  {
    key: "mayanew",
    id: "user_standinmayanew",
    name: "Maya Okafor",
    domain: "okafor.example",
    address: "maya.okafor@okafor.example",
    createdAt: STARTED,
  },
];
const deleted = new Set<string>();
const standing = () => PEOPLE.filter((p) => !deleted.has(p.id));

const port = Number(process.env.PORT ?? 4100);
const issuer = `http://127.0.0.1:${port}`;
const appOrigin = process.env.APP_ORIGIN ?? "http://127.0.0.1:5173";
const { publicKey, privateKey } = await generateKeyPair("RS256");
const jwks = {
  keys: [{ ...(await exportJWK(publicKey)), kid: "stand-in", alg: "RS256" }],
};

function token(userId: string) {
  return new SignJWT({ sid: `sess_${userId}`, azp: appOrigin, fva: [0, -1] })
    .setProtectedHeader({ alg: "RS256", kid: "stand-in", typ: "JWT" })
    .setIssuer(issuer)
    .setSubject(userId)
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(privateKey);
}

function user(person: (typeof PEOPLE)[number]) {
  const [first, ...rest] = person.name.split(" ");
  const email = {
    id: `idn_${person.key}`,
    email_address: person.address ?? `${person.key}@${person.domain}`,
    verification: { status: "verified" },
  };
  const more = ("alsoAt" in person ? person.alsoAt : []).map((domain) => ({
    id: `idn_${person.key}_${domain}`,
    email_address: `${person.key}@${domain}`,
    verification: { status: "verified" },
  }));
  return {
    id: person.id,
    first_name: first,
    last_name: rest.join(" "),
    primary_email_address_id: email.id,
    email_addresses: [email, ...more],
    created_at: person.createdAt ?? LONG_AGO,
  };
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", issuer);
  response.setHeader("access-control-allow-origin", appOrigin);
  if (url.pathname === "/pick") {
    response.setHeader("content-type", "text/html; charset=utf-8");
    response.end(
      `<!doctype html><title>Sign in · stand-in</title><body style="font:15px system-ui;background:#111;color:#ddd;padding:40px"><h1>Sign in as</h1>${standing()
        .map(
          (p) =>
            `<p><a style="color:#b9a6ff" href="${appOrigin}/?as=${p.key}">${p.name}</a> · ${p.address ?? `${p.key}@${p.domain}`}</p>`,
        )
        .join("")}</body>`,
    );
    return;
  }
  response.setHeader("content-type", "application/json");
  const send = (status: number, body: unknown) => {
    response.statusCode = status;
    response.end(JSON.stringify(body));
  };
  if (url.pathname === "/.well-known/jwks.json") return send(200, jwks);
  if (url.pathname === "/people") return send(200, standing());
  if (url.pathname === "/v1/users") {
    const ids = url.searchParams.getAll("user_id");
    return send(
      200,
      standing()
        .filter((p) => ids.includes(p.id))
        .map(user),
    );
  }
  const one = standing().find((p) => url.pathname === `/v1/users/${p.id}`);
  if (one && request.method === "DELETE") {
    deleted.add(one.id);
    return send(200, { id: one.id, deleted: true });
  }
  if (one) return send(200, user(one));
  if (url.pathname === "/token") {
    const person = standing().find((p) => p.key === url.searchParams.get("as"));
    if (!person) return send(404, { error: "no such person" });
    return send(200, { token: await token(person.id) });
  }
  send(404, { error: "not found" });
}).listen(port, "127.0.0.1", () =>
  console.log(`stand-in sign-in at ${issuer}`),
);
