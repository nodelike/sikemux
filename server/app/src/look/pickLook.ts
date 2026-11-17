import type { LookReply } from "@shared/wire.generated";

export type Answered = { seenAt: string; reply: LookReply };
type Look = Extract<LookReply, { result: "look" }>;

/**
 * The look of the host the person was at last: the one whose own window they were in front of
 * most recently, then the one that reached the accounts server last.
 */
export function pickLook(answered: Answered[]): Look | null {
  const looks = answered.flatMap(({ seenAt, reply }) =>
    reply.result === "look" ? [{ seenAt, look: reply }] : [],
  );
  looks.sort(
    (a, b) =>
      (b.look.personAtWindow ?? 0) - (a.look.personAtWindow ?? 0) ||
      b.seenAt.localeCompare(a.seenAt),
  );
  return looks[0]?.look ?? null;
}
