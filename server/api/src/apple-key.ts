import { createPrivateKey, type KeyObject } from "node:crypto";
import { readFileSync } from "node:fs";

import { settingsFrom } from "./env.ts";
import { signJwt } from "./jwt.ts";

/** A signing key Apple hands out as a .p8 file, and who it belongs to. */
export interface AppleKey {
  keyId: string;
  teamId: string;
  privateKey: KeyObject;
}

/** Apple's key and team ids are ten capital letters and digits. */
const APPLE_ID = /^[A-Z0-9]{10}$/;

/** Reads a .p8 key file, throwing a message that names what is wrong with it. */
export function readAppleKey(path: string): KeyObject {
  let pem: string;
  try {
    pem = readFileSync(path, "utf8");
  } catch (error) {
    throw new Error(
      `cannot be read (${(error as NodeJS.ErrnoException).code ?? "unknown error"})`,
      { cause: error },
    );
  }
  let key: KeyObject;
  try {
    key = createPrivateKey(pem);
  } catch {
    throw new Error("is not a PEM private key");
  }
  if (
    key.asymmetricKeyType !== "ec" ||
    key.asymmetricKeyDetails?.namedCurve !== "prime256v1"
  )
    throw new Error("is not a P-256 key, as Apple's .p8 keys are");
  return key;
}

/**
 * Reads `<prefix>_KEY_FILE`, the path of a .p8 key, with its `<prefix>_KEY_ID` and
 * `<prefix>_TEAM_ID`. Null when the file is not set, or when anything is wrong.
 */
export function readAppleKeySettings(
  env: NodeJS.ProcessEnv,
  prefix: string,
  problems: string[],
): AppleKey | null {
  const { optional } = settingsFrom(env, problems);
  const path = optional(`${prefix}_KEY_FILE`);
  if (!path) return null;
  const keyId = optional(`${prefix}_KEY_ID`) ?? "";
  const teamId = optional(`${prefix}_TEAM_ID`) ?? "";
  const before = problems.length;
  if (!APPLE_ID.test(keyId))
    problems.push(`${prefix}_KEY_ID is not a key id like ABC123DEFG`);
  if (!APPLE_ID.test(teamId))
    problems.push(`${prefix}_TEAM_ID is not a team id like D577WD6Z5U`);
  let privateKey: KeyObject | undefined;
  try {
    privateKey = readAppleKey(path);
  } catch (error) {
    problems.push(`${prefix}_KEY_FILE ${path} ${(error as Error).message}`);
  }
  if (!privateKey || problems.length > before) return null;
  return { keyId, teamId, privateKey };
}

/** An ES256 JSON Web Token signed with an Apple key, as APNs and Sign in with Apple both take. */
export function signAppleToken(key: AppleKey, claims: object): string {
  return signJwt({ alg: "ES256", kid: key.keyId }, claims, key.privateKey);
}
