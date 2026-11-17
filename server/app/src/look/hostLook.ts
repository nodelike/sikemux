import { ApiProblem } from "@shared/account/api";
import type { LookReply } from "@shared/wire.generated";
import device, {
  newKey,
  publicKey,
  readLook,
  signRegistration,
} from "@web-device";

import { api } from "../api.ts";

const ANSWER_MS = 15_000;

type Kept = { key: string; registered: boolean };

const hex = (bytes: Uint8Array) =>
  Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
const bytes = (text: string) =>
  Uint8Array.from(text.match(/../g) ?? [], (pair) => parseInt(pair, 16));

/** This browser's own device key on the account, kept apart per account. */
function kept(account: string): Kept {
  const name = `sikemux.device.${account}`;
  const stored = localStorage.getItem(name);
  if (stored) return JSON.parse(stored) as Kept;
  const made = { key: hex(newKey()), registered: false };
  localStorage.setItem(name, JSON.stringify(made));
  return made;
}

function keep(account: string, value: Kept) {
  localStorage.setItem(`sikemux.device.${account}`, JSON.stringify(value));
}

async function register(
  routes: ReturnType<typeof api>,
  account: string,
  key: Uint8Array,
) {
  const { nonce } = await routes.createChallenge();
  await routes.registerDevice({
    key: publicKey(key),
    role: "client",
    name: "Web app",
    platform: "web",
    nonce,
    signature: signRegistration(key, nonce, account),
  });
}

/**
 * The look of the host the person used last, read from it end to end over iroh as a phone reads it,
 * or null when no host answers.
 */
export async function readHostLook(
  account: string,
  token: string,
): Promise<LookReply | null> {
  await device();
  const routes = api(token);
  const [{ devices }, network] = await Promise.all([
    routes.listDevices({ role: "host" }),
    routes.getNetwork(),
  ]);
  const host = devices
    .flatMap((each) =>
      each.lastSeenAt ? [{ ...each, seen: each.lastSeenAt }] : [],
    )
    .sort((a, b) => b.seen.localeCompare(a.seen))[0];
  if (!host) return null;

  const own = kept(account);
  const key = bytes(own.key);
  const ticket = async () => {
    if (!own.registered) {
      await register(routes, account, key);
      keep(account, { ...own, registered: true });
      own.registered = true;
    }
    return routes.createJoinTicket(publicKey(key), { host: host.key });
  };
  const signed = await ticket().catch(async (error: unknown) => {
    if (!(error instanceof ApiProblem && error.status === 404)) throw error;
    own.registered = false;
    return ticket();
  });

  const answer = await Promise.race([
    readLook(
      key,
      network.relays.map((relay) => relay.url),
      host.key,
      JSON.stringify(signed),
    ),
    new Promise<never>((_, reject) =>
      setTimeout(
        () => reject(new Error("The host did not answer.")),
        ANSWER_MS,
      ),
    ),
  ]);
  return JSON.parse(answer) as LookReply;
}
