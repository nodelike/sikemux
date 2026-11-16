import { createServer } from "node:http";
import { exportJWK, generateKeyPair, SignJWT } from "jose";

/**
 * A sign-in for made-up people on a local run, so the signed-in app can be seen without a real
 * account. Point the local API's CLERK_ISSUER and the app's VITE_STAND_IN_CLERK at it.
 */
const PEOPLE = [
  { key: "ines", id: "user_standin_ines", name: "Inês Duarte" },
  { key: "tomas", id: "user_standin_tomas", name: "Tomás Reyes" },
  { key: "priya", id: "user_standin_priya", name: "Priya Natarajan" },
  { key: "jonah", id: "user_standin_jonah", name: "Jonah Lindqvist" },
  { key: "amara", id: "user_standin_amara", name: "Amara Okafor" },
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
  if (url.pathname === "/token") {
    const person = PEOPLE.find((p) => p.key === url.searchParams.get("as"));
    if (!person) return send(404, { error: "no such person" });
    return send(200, { token: await token(person.id) });
  }
  send(404, { error: "not found" });
}).listen(port, "127.0.0.1", () =>
  console.log(`stand-in sign-in at ${issuer}`),
);
