import { createHash, createPublicKey, verify } from "node:crypto";

/** The DER prefix that turns a raw 32-byte Ed25519 key into the form Node reads. */
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

/** What a device signs to register: binding the challenge, the account and the key together. */
export function registrationMessage(
  nonce: string,
  userId: string,
  key: string,
): string {
  return `sikemux-register|${nonce}|${userId}|${key}`;
}

/** What a device signs to open a live connection. The prefix keeps it from ever passing for a registration. */
export function liveMessage(nonce: string, key: string): string {
  return `sikemux-live|${nonce}|${key}`;
}

/** What a host signs to open a share connection. */
export function shareMessage(nonce: string, key: string): string {
  return `sikemux-share|${nonce}|${key}`;
}

/** What a phone signs to send its notifications to a token. The token goes in as its SHA-256, in hex. */
export function pushTokenMessage(
  nonce: string,
  key: string,
  token: string,
): string {
  const tokenHash = createHash("sha256").update(token, "utf8").digest("hex");
  return `sikemux-push|${nonce}|${key}|${tokenHash}`;
}

/** Whether `signature` (hex) is `key`'s (hex) Ed25519 signature over `message`. */
export function signedBy(
  key: string,
  message: string,
  signature: string,
): boolean {
  try {
    const publicKey = createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(key, "hex")]),
      format: "der",
      type: "spki",
    });
    return verify(
      null,
      Buffer.from(message, "utf8"),
      publicKey,
      Buffer.from(signature, "hex"),
    );
  } catch {
    return false;
  }
}
