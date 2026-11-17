import { ApiProblem } from "@shared/account/api";
import type { LookReply } from "@shared/wire.generated";
import device, {
  newKey,
  publicKey,
  readLook,
  signRegistration,
} from "@web-device";

import { api } from "../api.ts";
import { type Answered, pickLook } from "./pickLook.ts";

type Look = Extract<LookReply, { result: "look" }>;

const ANSWER_MS = 15_000;
/** How long the other hosts have to answer once one has. */
const OTHERS_MS = 3_000;

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
 * The look of the host the person was at last, read from each host end to end over iroh as a
 * phone reads it, or null when none answers.
 */
export async function readHostLook(
  account: string,
  token: string,
): Promise<Look | null> {
  await device();
  const routes = api(token);
  const [{ devices }, network] = await Promise.all([
    routes.listDevices({ role: "host" }),
    routes.getNetwork(),
  ]);
  const hosts = devices.flatMap((each) =>
    each.lastSeenAt ? [{ key: each.key, seenAt: each.lastSeenAt }] : [],
  );
  if (hosts.length === 0) return null;

  const own = kept(account);
  const key = bytes(own.key);
  const ticket = async (host: string) => {
    if (!own.registered) {
      await register(routes, account, key);
      keep(account, { ...own, registered: true });
      own.registered = true;
    }
    return routes.createJoinTicket(publicKey(key), { host });
  };
  const signed = (host: string) =>
    ticket(host).catch(async (error: unknown) => {
      if (!(error instanceof ApiProblem && error.status === 404)) throw error;
      own.registered = false;
      return ticket(host);
    });
  const relays = network.relays.map((relay) => relay.url);

  const answered: Answered[] = [];
  const asked = hosts.map(async (host) => {
    const answer = await readLook(
      key,
      relays,
      host.key,
      JSON.stringify(await signed(host.key)),
    );
    answered.push({
      seenAt: host.seenAt,
      reply: JSON.parse(answer) as LookReply,
    });
  });
  await new Promise<void>((done) => {
    const giveUp = setTimeout(done, ANSWER_MS);
    const first = Promise.any(asked).then(() => setTimeout(done, OTHERS_MS));
    void Promise.allSettled([...asked, first]).then(() => {
      clearTimeout(giveUp);
      done();
    });
  });
  return pickLook(answered);
}
