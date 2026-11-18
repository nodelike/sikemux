import { useAuth, useClerk } from "@clerk/react";
import { errorCode, orReverify } from "@shared/account/account";
import { ApiProblem } from "@shared/account/api";
import type {
  FoldCatch,
  FoldHint,
  FoldPreview,
  FoldWhy,
} from "@sikemux/protocol";
import { useEffect, useState } from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { Panel } from "./Consent.tsx";
import { JOIN, RETURN_KEY, SIGNED_UP_FOR_KEY } from "./navigation.ts";
import { useReverified } from "./Reverify.tsx";

const WAITING_KEY = "sikemux.fold";
const SIGNED_IN_HERE_KEY = "sikemux.signedInHere";
const settledKey = (userId: string) => `sikemux.oneAccount:${userId}`;

/** A fold this tab started, waiting for the person's other account to confirm it. */
type Waiting = {
  id: string;
  secret: string;
  expiresAt: string;
  why: FoldWhy;
  /** The address coming over, shown on the sign-in that confirms it. Absent when a seat comes over. */
  address?: string;
};

export function waitingFold(): Waiting | null {
  try {
    const waiting = JSON.parse(
      sessionStorage.getItem(WAITING_KEY) ?? "null",
    ) as Waiting | null;
    if (waiting && Date.parse(waiting.expiresAt) > Date.now()) return waiting;
  } catch {
    /* a value this page did not write reads as none */
  }
  sessionStorage.removeItem(WAITING_KEY);
  return null;
}

function dropWaiting() {
  sessionStorage.removeItem(WAITING_KEY);
}

function hintNow(): { hint?: FoldHint; inviteToken?: string } {
  if (location.pathname === JOIN && location.hash.length > 1)
    return { hint: "invite", inviteToken: location.hash.slice(1) };
  const returning = sessionStorage.getItem(RETURN_KEY);
  if (returning?.startsWith(`${JOIN}#`) && returning.length > JOIN.length + 1)
    return { hint: "invite", inviteToken: returning.slice(JOIN.length + 1) };
  const noted = sessionStorage.getItem(SIGNED_UP_FOR_KEY);
  if (noted === "share" || noted === "invite") return { hint: noted };
  if (localStorage.getItem(SIGNED_IN_HERE_KEY)) return { hint: "browser" };
  return {};
}

type Gate =
  | { kind: "checking" }
  | { kind: "open" }
  | { kind: "catch"; caught: FoldCatch; inviteToken?: string }
  | { kind: "confirm"; waiting: Waiting };

/**
 * Whether the signed-in account may go on to its pages: a new account may first be asked
 * whether the person already has one, and a fold this tab started is confirmed here.
 */
export function useFoldGate(signedIn: boolean) {
  const { getToken, userId } = useAuth();
  const [gate, setGate] = useState<Gate>({ kind: "checking" });

  useEffect(() => {
    if (!signedIn || !userId) return;
    let current = true;
    const { hint, inviteToken } = hintNow();
    void (async () => {
      const waiting = waitingFold();
      if (waiting) {
        setGate({ kind: "confirm", waiting });
        return;
      }
      if (localStorage.getItem(settledKey(userId))) {
        setGate({ kind: "open" });
        return;
      }
      try {
        const token = await signedInToken(getToken, "see your account");
        const caught = await api(token).getFoldCatch({
          ...(hint ? { hint } : {}),
          ...(inviteToken ? { inviteToken } : {}),
        });
        if (!current) return;
        if (caught.ask) {
          setGate({ kind: "catch", caught, inviteToken });
          return;
        }
        localStorage.setItem(settledKey(userId), "1");
      } catch {
        /* the pages say what went wrong */
      }
      if (!current) return;
      sessionStorage.removeItem(SIGNED_UP_FOR_KEY);
      localStorage.setItem(SIGNED_IN_HERE_KEY, "1");
      setGate({ kind: "open" });
    })();
    return () => {
      current = false;
    };
  }, [signedIn, userId, getToken]);

  const open = () => {
    if (userId) localStorage.setItem(settledKey(userId), "1");
    sessionStorage.removeItem(SIGNED_UP_FOR_KEY);
    localStorage.setItem(SIGNED_IN_HERE_KEY, "1");
    setGate({ kind: "open" });
  };

  const screen =
    gate.kind === "catch" ? (
      <Catch caught={gate.caught} inviteToken={gate.inviteToken} onNew={open} />
    ) : gate.kind === "confirm" ? (
      <ConfirmFold
        waiting={gate.waiting}
        onDone={open}
        onCancel={() => {
          dropWaiting();
          setGate({ kind: "checking" });
          location.reload();
        }}
      />
    ) : gate.kind === "checking" ? (
      <Panel>
        <p className="quiet">Loading…</p>
      </Panel>
    ) : null;

  return { open: gate.kind === "open", screen };
}

/** Starts a fold on this tab, then signs out so the person signs in with their other account. */
function useStartFold() {
  const { getToken } = useAuth();
  const { signOut } = useClerk();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const { run, confirm } = useReverified(
    async (why: FoldWhy, inviteToken?: string) => {
      const token = await signedInToken(
        getToken,
        "bring your accounts together",
        {
          fresh: true,
        },
      );
      return orReverify(() =>
        api(token).startFold({
          why,
          ...(inviteToken ? { inviteToken } : {}),
        }),
      );
    },
  );

  const start = async (
    why: FoldWhy,
    { address, inviteToken }: { address?: string; inviteToken?: string } = {},
  ) => {
    setBusy(true);
    setProblem(undefined);
    try {
      const intent = await run(why, inviteToken);
      const waiting: Waiting = {
        id: intent.id,
        secret: intent.secret,
        expiresAt: intent.expiresAt,
        why,
        ...(address ? { address } : {}),
      };
      sessionStorage.setItem(WAITING_KEY, JSON.stringify(waiting));
      if (inviteToken)
        sessionStorage.setItem(RETURN_KEY, `${JOIN}#${inviteToken}`);
      await signOut({ redirectUrl: "/" });
    } catch (error) {
      setBusy(false);
      if (errorCode(error) === "reverification_cancelled") return;
      setProblem(problemText(error));
    }
  };
  return { start, busy, problem, confirm };
}

function Catch({
  caught,
  inviteToken,
  onNew,
}: {
  caught: FoldCatch;
  inviteToken?: string;
  onNew: () => void;
}) {
  const { getToken } = useAuth();
  const { signOut } = useClerk();
  const { start, busy, problem, confirm } = useStartFold();
  const [keeping, setKeeping] = useState(false);
  const [keepProblem, setKeepProblem] = useState<string>();
  const address = caught.address ?? "this address";

  if (confirm) return confirm;

  const keepNew = async () => {
    setKeeping(true);
    setKeepProblem(undefined);
    try {
      const token = await signedInToken(getToken, "keep this account");
      await api(token).keepNewAccount();
      onNew();
    } catch (error) {
      setKeeping(false);
      setKeepProblem(problemText(error));
    }
  };

  const why = caught.why ?? "browser";
  return (
    <Panel>
      <h1>
        {caught.mustFold
          ? why === "domain"
            ? `${caught.orgName ?? "An org"} uses this address`
            : "This address is already on Sikemux"
          : "Already on Sikemux?"}
      </h1>
      <div className="consent-who">
        <span className="ink">{address}</span>
      </div>
      <p className="lede">
        {caught.mustFold
          ? why === "domain"
            ? `${caught.orgName ?? "An org"} has proven this address's domain, so it doesn't open a Sikemux account of its own. Sign in with your own Google, Apple or email instead, and the address comes along to prove your place in ${caught.orgName ?? "the org"}. It never signs you in.`
            : "Another Sikemux account already proved this address. Sign in with that account's own Google, Apple or email, and this new account folds into it."
          : `You just made a new account with this address${
              why === "invite" && caught.orgName
                ? ` to join ${caught.orgName}`
                : ""
            }. If you already have a Sikemux account, sign in with it and bring the address over, so you keep one account.`}
      </p>
      {problem || keepProblem ? (
        <p className="problem" role="alert">
          {problem ?? keepProblem}
        </p>
      ) : null}
      <div className="consent-actions">
        {caught.mustFold ? (
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => void signOut({ redirectUrl: "/" })}
          >
            Sign out
          </button>
        ) : (
          <button
            type="button"
            className="button"
            disabled={busy || keeping}
            data-busy={keeping || undefined}
            onClick={() => void keepNew()}
          >
            I'm new here
          </button>
        )}
        <button
          type="button"
          className="button primary"
          disabled={busy || keeping}
          data-busy={busy || undefined}
          onClick={() =>
            void start(why, {
              ...(caught.address ? { address: caught.address } : {}),
              ...(why === "invite" && inviteToken ? { inviteToken } : {}),
            })
          }
        >
          Use my account
        </button>
      </div>
    </Panel>
  );
}

/** Above the sign-in, while a fold waits for the person's other account. */
export function FoldWaiting() {
  const [waiting, setWaiting] = useState(waitingFold);
  if (!waiting) return null;
  return (
    <section className="panel fold-waiting">
      <p className="lede">
        {waiting.why === "seat"
          ? "Sign in with the account that holds the seat, to bring it to your own."
          : `Sign in with your own account to bring ${waiting.address ?? "the address"} to it.`}{" "}
        <a
          href="/"
          onClick={(event) => {
            event.preventDefault();
            dropWaiting();
            setWaiting(null);
          }}
        >
          Cancel
        </a>
      </p>
    </section>
  );
}

type Read =
  | { state: "loading" }
  | { state: "failed"; problem: string; sameAccount: boolean }
  | { state: "loaded"; preview: FoldPreview }
  | { state: "done"; kept: string };

function ConfirmFold({
  waiting,
  onDone,
  onCancel,
}: {
  waiting: Waiting;
  onDone: () => void;
  onCancel: () => void;
}) {
  const { getToken } = useAuth();
  const { signOut } = useClerk();
  const [read, setRead] = useState<Read>({ state: "loading" });
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const { run, confirm } = useReverified(async () => {
    const token = await signedInToken(
      getToken,
      "bring your accounts together",
      {
        fresh: true,
      },
    );
    return orReverify(() =>
      api(token).confirmFold(waiting.id, { secret: waiting.secret }),
    );
  });

  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const token = await signedInToken(getToken, "see your account");
        const preview = await api(token).previewFold(waiting.id, {
          secret: waiting.secret,
        });
        if (current) setRead({ state: "loaded", preview });
      } catch (error) {
        if (!current) return;
        const status = error instanceof ApiProblem ? error.status : undefined;
        if (status === 404) dropWaiting();
        setRead({
          state: "failed",
          problem: problemText(error),
          sameAccount: status === 409 && waiting.why !== "seat",
        });
      }
    })();
    return () => {
      current = false;
    };
  }, [getToken, waiting]);

  if (confirm) return confirm;

  if (read.state === "loading")
    return (
      <Panel>
        <p className="quiet">Loading…</p>
      </Panel>
    );

  if (read.state === "failed")
    return (
      <Panel>
        <h1>Nothing was brought over</h1>
        <p className="problem" role="alert">
          {read.problem}
        </p>
        <div className="consent-actions">
          {read.sameAccount ? (
            <button
              type="button"
              className="button primary"
              onClick={() => void signOut({ redirectUrl: "/" })}
            >
              Sign in with another account
            </button>
          ) : null}
          <button type="button" className="button" onClick={onCancel}>
            {read.sameAccount ? "Cancel" : "Continue"}
          </button>
        </div>
      </Panel>
    );

  const seatComing = waiting.why === "seat";

  if (read.state === "done")
    return (
      <Panel>
        <h1>{seatComing ? "Your seat moved" : "Brought over"}</h1>
        <p className="lede">
          {seatComing
            ? `This account is deleted, and its seats are on ${read.kept} now. Sign in with that account from here on.`
            : "This account proves the address now. The account made with it is deleted, and any seat it held is here."}
        </p>
        <div className="consent-actions">
          <button
            type="button"
            className="button primary"
            onClick={() =>
              seatComing ? void signOut({ redirectUrl: "/team" }) : onDone()
            }
          >
            {seatComing ? "Sign in" : "Continue"}
          </button>
        </div>
      </Panel>
    );

  const { preview } = read;
  const kept = preview.keptName ?? "your own account";
  const seats =
    preview.seats.length > 0
      ? `Its seat${preview.seats.length > 1 ? "s" : ""} in ${preview.seats.join(", ")} move${preview.seats.length > 1 ? "" : "s"} to ${seatComing ? kept : "this account"}.`
      : "";

  const bring = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await run();
      dropWaiting();
      setRead({ state: "done", kept });
    } catch (error) {
      setBusy(false);
      if (errorCode(error) === "reverification_cancelled") return;
      setProblem(problemText(error));
    }
  };

  return (
    <Panel>
      <h1>
        {seatComing
          ? `Move this account's seat to ${kept}?`
          : "Bring this address to this account?"}
      </h1>
      <div className="consent-who">
        <span className="ink">{preview.address}</span>
      </div>
      <p className="lede">
        {seatComing
          ? `This account, made with the address above, holds nothing but its seat. ${seats} Then this account is deleted, and you sign in as ${kept} from here on.`
          : `The account made with this address holds nothing but ${preview.seats.length ? "a seat" : "its sign-in"}. ${seats} It is then deleted, and this account proves the address${preview.orgName ? ` for ${preview.orgName}` : ""}. The address never signs you in: keep signing in as you do now.`}
      </p>
      {problem ? (
        <p className="problem" role="alert">
          {problem}
        </p>
      ) : null}
      <div className="consent-actions">
        <button
          type="button"
          className="button"
          disabled={busy}
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          type="button"
          className="button primary"
          disabled={busy}
          data-busy={busy || undefined}
          onClick={() => void bring()}
        >
          {seatComing ? "Move the seat" : "Bring it here"}
        </button>
      </div>
    </Panel>
  );
}

/** "Bring it here" on the Team page: a seat another of the person's accounts holds. */
export function BringSeat() {
  const { start, busy, problem, confirm } = useStartFold();
  if (confirm) return confirm;
  return (
    <>
      <button
        type="button"
        className="button small"
        disabled={busy}
        data-busy={busy || undefined}
        onClick={() => void start("seat")}
      >
        Bring it here
      </button>
      {problem ? (
        <p className="problem" role="alert">
          {problem}
        </p>
      ) : null}
    </>
  );
}
