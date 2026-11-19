import { createHash, randomBytes } from "node:crypto";
import { createServer, type IncomingMessage } from "node:http";
import { exportJWK, generateKeyPair, SignJWT } from "jose";

/**
 * A sign-in for made-up people on a local run, so the signed-in app can be seen without a real
 * account. Point the local API's CLERK_ISSUER and the app's VITE_STAND_IN_CLERK at it. It also
 * answers the Backend API's user reads and deletes, so the API can name them and fold them away.
 * Jonah added his lumen.test address months after signing up, before any of this checking.
 * Under /google it is Google too, for proving a work address, signing in whatever address is typed.
 * Maya and Kofi each have two accounts: a personal one, and one made with a work address at
 * acme.example, Maya's just now. Maya also has a third, made just now with another address of
 * her own, for the catch that only asks.
 */
const LONG_AGO = Date.parse("2025-06-01T09:00:00Z");
const ADDED_LATER = Date.parse("2025-09-01T09:00:00Z");
const STARTED = Date.now();

const PEOPLE: {
  key: string;
  id: string;
  name: string;
  domain: string;
  address?: string;
  alsoAt?: string[];
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

type Address = {
  id: string;
  email_address: string;
  created_at: number;
  verification: { status: string };
  linked_to: { type: string; id: string }[];
};
type Account = { id: string; provider: string; email_address: string };
type Held = { primary: string; addresses: Address[]; accounts: Account[] };

const CODE = "424242";
const held = new Map<string, Held>();
let made = 0;

/** Each person's addresses and connected accounts, which the app's stand-in may change. */
function holding(person: (typeof PEOPLE)[number]): Held {
  let kept = held.get(person.key);
  if (!kept) {
    const domains = [person.domain, ...(person.alsoAt ?? [])];
    const addresses = domains.map((domain, index) => ({
      id:
        index === 0
          ? `idn_${person.key}`
          : `idn_${person.key}${domain.replace(/\W/g, "")}`,
      email_address:
        index === 0
          ? (person.address ?? `${person.key}@${domain}`)
          : `${person.key}@${domain}`,
      created_at: index === 0 ? (person.createdAt ?? LONG_AGO) : ADDED_LATER,
      verification: { status: "verified" },
      linked_to: [],
    }));
    kept = { primary: `idn_${person.key}`, addresses, accounts: [] };
    held.set(person.key, kept);
  }
  return kept;
}

function user(person: (typeof PEOPLE)[number]) {
  const [first, ...rest] = person.name.split(" ");
  const kept = holding(person);
  return {
    id: person.id,
    first_name: first,
    last_name: rest.join(" "),
    primary_email_address_id: kept.primary,
    email_addresses: kept.addresses,
    created_at: person.createdAt ?? LONG_AGO,
    external_accounts: kept.accounts.map((account) => ({
      ...account,
      provider: `oauth_${account.provider}`,
    })),
  };
}

function connectPage(key: string, provider: string, back: string) {
  return `<!doctype html><meta charset="utf-8"><title>Stand-in ${provider}</title>
<form method="post" action="/connect" style="font:15px system-ui;margin:4rem auto;max-width:22rem;display:grid;gap:.75rem">
<p>Stand-in ${provider}: which address does this account have?</p>
<input name="email" type="email" required autofocus value="${key}@acme.example">
<input type="hidden" name="as" value="${key}"><input type="hidden" name="provider" value="${provider}">
<input type="hidden" name="back" value="${back.replace(/"/g, "&quot;")}">
<button>Continue</button></form>`;
}

type GoogleCode = {
  email: string;
  nonce: string;
  challenge: string;
  clientId: string;
  redirectUri: string;
};
const googleCodes = new Map<string, GoogleCode>();

function googlePage(asked: URLSearchParams) {
  const hidden = [
    "client_id",
    "redirect_uri",
    "state",
    "nonce",
    "code_challenge",
  ]
    .map(
      (name) =>
        `<input type="hidden" name="${name}" value="${(asked.get(name) ?? "").replace(/"/g, "&quot;")}">`,
    )
    .join("");
  return `<!doctype html><meta charset="utf-8"><title>Stand-in Google</title>
<form method="post" action="/google/authorize" style="font:15px system-ui;margin:4rem auto;max-width:22rem;display:grid;gap:.75rem">
<p>Stand-in Google: which address signs in?</p>
<input name="email" type="email" required autofocus value="maya@acme.example">${hidden}
<button>Continue</button></form>`;
}

async function google(
  path: string,
  method: string,
  request: IncomingMessage,
  url: URL,
) {
  if (path === "/keys") return { status: 200, body: jwks };
  if (path === "/authorize" && method === "GET")
    return { html: googlePage(url.searchParams) };
  if (path === "/authorize" && method === "POST") {
    const form = await bodyOf(request);
    const code = randomBytes(16).toString("base64url");
    googleCodes.set(code, {
      email: form.email ?? "",
      nonce: form.nonce ?? "",
      challenge: form.code_challenge ?? "",
      clientId: form.client_id ?? "",
      redirectUri: form.redirect_uri ?? "",
    });
    const back = new URL(form.redirect_uri ?? appOrigin);
    back.searchParams.set("code", code);
    back.searchParams.set("state", form.state ?? "");
    return { redirect: back.toString() };
  }
  if (path === "/token" && method === "POST") {
    const form = await bodyOf(request);
    const asked = googleCodes.get(form.code ?? "");
    googleCodes.delete(form.code ?? "");
    const challenge = createHash("sha256")
      .update(form.code_verifier ?? "")
      .digest("base64url");
    if (
      !asked ||
      asked.challenge !== challenge ||
      asked.clientId !== form.client_id ||
      asked.redirectUri !== form.redirect_uri
    )
      return { status: 400, body: { error: "invalid_grant" } };
    const idToken = await new SignJWT({
      email: asked.email,
      email_verified: true,
      nonce: asked.nonce,
    })
      .setProtectedHeader({ alg: "RS256", kid: "stand-in", typ: "JWT" })
      .setIssuer(`${issuer}/google`)
      .setAudience(asked.clientId)
      .setSubject(`google-${asked.email}`)
      .setIssuedAt()
      .setExpirationTime("1h")
      .sign(privateKey);
    return { status: 200, body: { id_token: idToken } };
  }
  return { status: 404, body: { error: "not found" } };
}

async function bodyOf(request: IncomingMessage) {
  let text = "";
  for await (const chunk of request) text += chunk;
  if ((request.headers["content-type"] ?? "").includes("json"))
    return JSON.parse(text || "{}") as Record<string, string>;
  return Object.fromEntries(new URLSearchParams(text));
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", issuer);
  const method = request.method ?? "GET";
  response.setHeader("access-control-allow-origin", appOrigin);
  if (url.pathname === "/pick") {
    response.setHeader("content-type", "text/html; charset=utf-8");
    response.end(
      `<!doctype html><title>Sign in · stand-in</title><body style="font:15px system-ui;background:#111;color:#ddd;padding:40px"><h1>Sign in as</h1>${standing()
        .map(
          (p) =>
            `<p><a style="color:#b9a6ff" href="${appOrigin}/?as=${p.key}">${p.name}</a> · ${p.address ?? `${p.key}@${p.domain}`}</p>`,
        )
        .join(
          "",
        )}<p><a style="color:#b9a6ff" href="${appOrigin}/?as=out">Nobody</a> · the signed-out page</p></body>`,
    );
    return;
  }
  response.setHeader(
    "access-control-allow-methods",
    "GET, POST, PATCH, DELETE",
  );
  response.setHeader("access-control-allow-headers", "content-type");
  response.setHeader("content-type", "application/json");
  const send = (status: number, body: unknown) => {
    response.statusCode = status;
    response.end(JSON.stringify(body));
  };
  if (method === "OPTIONS") return send(204, null);
  if (url.pathname.startsWith("/google/")) {
    const answer = await google(
      url.pathname.slice("/google".length),
      method,
      request,
      url,
    );
    if ("html" in answer) {
      response.setHeader("content-type", "text/html; charset=utf-8");
      response.end(answer.html);
    } else if ("redirect" in answer) {
      response.statusCode = 303;
      response.setHeader("location", answer.redirect);
      response.end();
    } else send(answer.status, answer.body);
    return;
  }
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
  const byId = standing().find(
    (p) =>
      url.pathname === `/v1/users/${p.id}` ||
      url.pathname.startsWith(`/v1/users/${p.id}/`),
  );
  if (byId) {
    const kept = holding(byId);
    const rest = url.pathname.slice(`/v1/users/${byId.id}`.length);
    if (rest === "" && method === "PATCH") {
      kept.primary =
        (await bodyOf(request)).primary_email_address_id ?? kept.primary;
      return send(200, user(byId));
    }
    const account = rest.match(/^\/external_accounts\/(.+)$/);
    if (account && method === "DELETE") {
      kept.accounts = kept.accounts.filter((a) => a.id !== account[1]);
      return send(200, { deleted: true });
    }
    if (rest === "") return send(200, user(byId));
  }
  const address = url.pathname.match(/^\/v1\/email_addresses\/(.+)$/);
  if (address && method === "DELETE") {
    for (const kept of held.values())
      kept.addresses = kept.addresses.filter((a) => a.id !== address[1]);
    return send(200, { deleted: true });
  }

  const me = standing().find((p) => p.key === url.searchParams.get("as"));
  if (url.pathname === "/token") {
    if (!me) return send(404, { error: "no such person" });
    return send(200, { token: await token(me.id) });
  }
  if (url.pathname.startsWith("/me") && me) {
    const kept = holding(me);
    if (url.pathname === "/me") return send(200, user(me));
    if (url.pathname === "/me/email_addresses" && method === "POST") {
      const added: Address = {
        id: `idn_${me.key}added${++made}`,
        email_address: (await bodyOf(request)).email ?? "",
        created_at: Date.now(),
        verification: { status: "unverified" },
        linked_to: [],
      };
      kept.addresses.push(added);
      return send(200, added);
    }
    const verify = url.pathname.match(/^\/me\/email_addresses\/(.+)\/verify$/);
    if (verify) {
      const found = kept.addresses.find((a) => a.id === verify[1]);
      if (!found || (await bodyOf(request)).code !== CODE)
        return send(422, {
          message: `That code is wrong. The stand-in's code is ${CODE}.`,
        });
      found.verification.status = "verified";
      return send(200, found);
    }
    const gone = url.pathname.match(
      /^\/me\/(email_addresses|external_accounts)\/(.+)$/,
    );
    if (gone && method === "DELETE") {
      if (gone[1] === "email_addresses")
        kept.addresses = kept.addresses.filter((a) => a.id !== gone[2]);
      else kept.accounts = kept.accounts.filter((a) => a.id !== gone[2]);
      return send(200, { deleted: true });
    }
  }
  if (url.pathname === "/connect" && method === "GET" && me) {
    response.setHeader("content-type", "text/html");
    response.end(
      connectPage(
        me.key,
        url.searchParams.get("provider") ?? "google",
        url.searchParams.get("back") ?? appOrigin,
      ),
    );
    return;
  }
  if (url.pathname === "/connect" && method === "POST") {
    const {
      as = "",
      provider = "google",
      email = "",
      back = appOrigin,
    } = await bodyOf(request);
    const form = { as, provider, email, back };
    const person = standing().find((p) => p.key === form.as);
    if (!person) return send(404, { error: "no such person" });
    const kept = holding(person);
    const id = `eac_${person.key}${form.provider}${++made}`;
    kept.accounts.push({
      id,
      provider: form.provider,
      email_address: form.email,
    });
    if (!kept.addresses.some((a) => a.email_address === form.email))
      kept.addresses.push({
        id: `idn_${person.key}linked${made}`,
        email_address: form.email,
        created_at: Date.now(),
        verification: { status: "verified" },
        linked_to: [{ type: `oauth_${form.provider}`, id }],
      });
    response.statusCode = 303;
    response.setHeader("location", form.back);
    response.end();
    return;
  }
  send(404, { error: "not found" });
}).listen(port, "127.0.0.1", () =>
  console.log(`stand-in sign-in at ${issuer}`),
);
