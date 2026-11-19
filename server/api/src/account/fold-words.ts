import type { FoldWhy } from "@sikemux/protocol";

/** What the web app and both phones say about a fold, so the three never word it apart. */

export function catchWords({
  mustFold,
  why,
  address,
  orgName,
}: {
  mustFold: boolean;
  why: FoldWhy;
  address: string;
  orgName: string | undefined;
}): { title: string; words: string } {
  if (mustFold && why === "domain")
    return {
      title: `This is an ${orgName ?? "org"} address`,
      words: `Addresses at ${address.split("@")[1] ?? "this domain"} belong to ${orgName ?? "your org"}, so one can't have a Sikemux account of its own. Sign in with your own Google, Apple or email, and this address comes along to show you belong to ${orgName ?? "the org"}. It never signs you in.`,
    };
  if (mustFold)
    return {
      title: "This address is already on Sikemux",
      words:
        "This address is already verified on a Sikemux account, so it can't have one of its own. Sign in to that account with its own Google, Apple or email, and this new one is removed.",
    };
  return {
    title: "Already on Sikemux?",
    words: `You just made a new account with this address${
      why === "invite" && orgName ? ` to join ${orgName}` : ""
    }. If you already have a Sikemux account, sign in with it and bring the address over, so you keep one account.`,
  };
}

/** The line above the sign-in while a fold waits for the person's other account. */
export function waitingWords(why: FoldWhy): string {
  return why === "seat"
    ? "Sign in with the account that holds the seat, to bring it to your own."
    : "Sign in with your own account to bring over the address you just used.";
}

export function previewWords({
  why,
  kept: keptAccount,
  orgName,
  seats: seatOrgs,
}: {
  why: FoldWhy;
  kept: string | null;
  orgName: string | null;
  seats: string[];
}): { title: string; words: string; doneTitle: string; doneWords: string } {
  const seatComing = why === "seat";
  const kept = keptAccount ?? "your own account";
  const seats =
    seatOrgs.length > 0
      ? `Its seat${seatOrgs.length > 1 ? "s" : ""} in ${seatOrgs.join(", ")} move${seatOrgs.length > 1 ? "" : "s"} to ${seatComing ? kept : "this account"}.`
      : "";
  return seatComing
    ? {
        title: `Move this account's seat to ${kept}?`,
        words: `This account, made with the address above, holds nothing but its seat. ${seats} Then this account is deleted, and you sign in as ${kept} from here on.`,
        doneTitle: "Your seat moved",
        doneWords: `This account is deleted, and its seats are on ${kept} now. Sign in with that account from here on.`,
      }
    : {
        title: "Bring this address to this account?",
        words: `The account you just made with it holds nothing but ${seatOrgs.length ? "a seat" : "its sign-in"}. ${seats} It's deleted, and the address is verified${orgName ? ` for ${orgName}` : ""} on this account. The address never signs you in: keep signing in as you do now.`,
        doneTitle: "Brought over",
        doneWords:
          "The address is verified on this account now. The account made with it is deleted, and any seat it held is here.",
      };
}
