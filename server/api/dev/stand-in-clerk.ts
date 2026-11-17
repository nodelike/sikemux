import { createServer } from "node:http";
import { exportJWK, generateKeyPair, SignJWT } from "jose";

/**
 * A sign-in for made-up people on a local run, so the signed-in app can be seen without a real
 * account. Point the local API's CLERK_ISSUER and the app's VITE_STAND_IN_CLERK at it. It also
 * answers the Backend API's user reads, so the API can name them.
 */
const PEOPLE = [
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
  },
  {
    key: "amara",
    id: "user_standinamara",
    name: "Amara Okafor",
    domain: "lumen.test",
  },
];

const port = Number(process.env.PORT ?? 4100);
const issuer = `http://127.0.0.1:${port}`;
const appOrigin = process.env.APP_ORIGIN ?? "http://127.0.0.1:5173";
const { publicKey, privateKey } = await generateKeyPair("RS256");
const jwks = {
  keys: [{ ...(await exportJWK(publicKey)), kid: "stand-in", alg: "RS256" }],
};

function token(userId: string) {
  return new SignJWT({ sid: `sess_${userId}`, azp: appOrigin })
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
    email_address: `${person.key}@${person.domain}`,
    verification: { status: "verified" },
  };
  return {
    id: person.id,
    first_name: first,
    last_name: rest.join(" "),
    primary_email_address_id: email.id,
    email_addresses: [email],
  };
}

createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", issuer);
  response.setHeader("access-control-allow-origin", appOrigin);
  response.setHeader("content-type", "application/json");
  const send = (status: number, body: unknown) => {
    response.statusCode = status;
    response.end(JSON.stringify(body));
  };
  if (url.pathname === "/.well-known/jwks.json") return send(200, jwks);
  if (url.pathname === "/people") return send(200, PEOPLE);
  if (url.pathname === "/v1/users") {
    const ids = url.searchParams.getAll("user_id");
    return send(200, PEOPLE.filter((p) => ids.includes(p.id)).map(user));
  }
  const one = PEOPLE.find((p) => url.pathname === `/v1/users/${p.id}`);
  if (one) return send(200, user(one));
  if (url.pathname === "/token") {
    const person = PEOPLE.find((p) => p.key === url.searchParams.get("as"));
    if (!person) return send(404, { error: "no such person" });
    return send(200, { token: await token(person.id) });
  }
  send(404, { error: "not found" });
}).listen(port, "127.0.0.1", () =>
  console.log(`stand-in sign-in at ${issuer}`),
);
