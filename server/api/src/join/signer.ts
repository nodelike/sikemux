import {
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  sign,
  type KeyObject,
} from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import type { JoinTicket, PeerTicket } from "@sikemux/protocol";

import { settingsFrom } from "../env.ts";

export interface JoinSigner {
  keyId: string;
  privateKey: KeyObject;
  /** The raw 32-byte Ed25519 public key in lowercase hex, as hosts trust it. */
  publicKey: string;
  file: string;
  /** Whether this start made the dev key, rather than finding it. */
  created: boolean;
}

const TICKET_SECONDS = 600;
const PEER_TICKET_SECONDS = 300;
const KEY_ID = /^[a-z0-9]+(-[a-z0-9]+)*$/;

export function devKeyFile(home: string): string {
  return join(home, ".config/sikemux/dev/join-signing-key.pem");
}

/** What the server signs for a ticket. Times are unix seconds in decimal, keys lowercase hex. */
export function joinMessage(ticket: Omit<JoinTicket, "signature">): string {
  const base = `sikemux-join|v${ticket.v}|${ticket.keyId}|${ticket.account}|${ticket.host}|${ticket.phone}|${ticket.issuedAt}|${ticket.expiresAt}`;
  const via =
    ticket.via === undefined
      ? base
      : `${base}|${ticket.via}|${ticket.viaName ?? ""}`;
  const hands = ticket.handsOnly ? `${via}|hands-only` : via;
  return ticket.lookOnly ? `${hands}|look-only` : hands;
}

export function signTicket(
  signer: Pick<JoinSigner, "keyId" | "privateKey">,
  {
    account,
    host,
    phone,
    via,
    handsOnly,
    lookOnly,
  }: Pick<JoinTicket, "account" | "host" | "phone"> & {
    via?: { key: string; name: string };
    handsOnly?: boolean;
    lookOnly?: boolean;
  },
  now = Date.now(),
): JoinTicket {
  const issuedAt = Math.floor(now / 1000);
  const fields: Omit<JoinTicket, "signature"> = {
    v: 1,
    keyId: signer.keyId,
    account,
    host,
    phone,
    issuedAt,
    expiresAt: issuedAt + TICKET_SECONDS,
  };
  if (via) {
    fields.via = via.key;
    fields.viaName = via.name;
  }
  if (handsOnly) fields.handsOnly = true;
  if (lookOnly) fields.lookOnly = true;
  const signature = sign(
    null,
    Buffer.from(joinMessage(fields), "utf8"),
    signer.privateKey,
  ).toString("hex");
  return { ...fields, signature };
}

/** What the server signs for a peer ticket. The name goes last, so no character in it can shift a field. */
export function peerMessage(ticket: Omit<PeerTicket, "signature">): string {
  return `sikemux-peer|v${ticket.v}|${ticket.keyId}|${ticket.org}|${ticket.member}|${ticket.from}|${ticket.to}|${ticket.issuedAt}|${ticket.expiresAt}|${ticket.memberName}`;
}

export function signPeerTicket(
  signer: Pick<JoinSigner, "keyId" | "privateKey">,
  fields: Pick<PeerTicket, "org" | "member" | "memberName" | "from" | "to">,
  now = Date.now(),
): PeerTicket {
  const issuedAt = Math.floor(now / 1000);
  const unsigned: Omit<PeerTicket, "signature"> = {
    v: 1,
    keyId: signer.keyId,
    ...fields,
    issuedAt,
    expiresAt: issuedAt + PEER_TICKET_SECONDS,
  };
  const signature = sign(
    null,
    Buffer.from(peerMessage(unsigned), "utf8"),
    signer.privateKey,
  ).toString("hex");
  return { ...unsigned, signature };
}

export function rawPublicKey(privateKey: KeyObject): string {
  return createPublicKey(privateKey)
    .export({ format: "der", type: "spki" })
    .subarray(12)
    .toString("hex");
}

function readKey(file: string): KeyObject {
  const privateKey = createPrivateKey(readFileSync(file, "utf8"));
  if (privateKey.asymmetricKeyType !== "ed25519")
    throw new Error("is not an Ed25519 private key");
  return privateKey;
}

function createDevKey(file: string): boolean {
  mkdirSync(dirname(file), { recursive: true, mode: 0o700 });
  const { privateKey } = generateKeyPairSync("ed25519");
  try {
    writeFileSync(
      file,
      privateKey.export({ format: "pem", type: "pkcs8" }) as string,
      { mode: 0o600, flag: "wx" },
    );
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "EEXIST") return false;
    throw error;
  }
}

/**
 * Reads JOIN_SIGNING_KEY_FILE, a PKCS8 Ed25519 key in PEM, and JOIN_SIGNING_KEY_ID. A release
 * build refuses to start without the file; any other build makes and keeps a dev key in the
 * home folder, where dev builds of the core read its public key.
 */
export function readJoinSigner(
  env: NodeJS.ProcessEnv,
  problems: string[],
  release: boolean,
): JoinSigner | null {
  const { optional, text } = settingsFrom(env, problems);
  const keyId = text("JOIN_SIGNING_KEY_ID", release ? "prod-1" : "dev-1");
  if (!KEY_ID.test(keyId) || keyId.length > 32)
    problems.push(
      "JOIN_SIGNING_KEY_ID is not lowercase words joined by dashes, like prod-1",
    );

  const configured = optional("JOIN_SIGNING_KEY_FILE");
  if (!configured && release) {
    problems.push("JOIN_SIGNING_KEY_FILE is not set");
    return null;
  }
  const file = configured ?? devKeyFile(optional("HOME") ?? homedir());
  let created = false;
  try {
    if (!configured) created = createDevKey(file);
    const privateKey = readKey(file);
    return {
      keyId,
      privateKey,
      publicKey: rawPublicKey(privateKey),
      file,
      created,
    };
  } catch (error) {
    problems.push(
      `JOIN_SIGNING_KEY_FILE ${file} cannot be used: ${(error as Error).message}`,
    );
    return null;
  }
}
