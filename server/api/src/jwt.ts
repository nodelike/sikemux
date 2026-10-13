import { sign, type KeyObject } from "node:crypto";

export interface JwtHeader {
  alg: "ES256" | "RS256";
  kid?: string;
  typ?: "JWT";
}

function base64url(value: string | Buffer): string {
  return Buffer.from(value).toString("base64url");
}

/** A JSON Web Token signed with SHA-256, for Apple's and Google's servers. */
export function signJwt(
  header: JwtHeader,
  claims: object,
  privateKey: KeyObject,
): string {
  const unsigned = `${base64url(JSON.stringify(header))}.${base64url(JSON.stringify(claims))}`;
  const signature = sign(
    "sha256",
    Buffer.from(unsigned),
    header.alg === "ES256"
      ? { key: privateKey, dsaEncoding: "ieee-p1363" }
      : privateKey,
  );
  return `${unsigned}.${base64url(signature)}`;
}
