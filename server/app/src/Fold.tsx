import { useAuth, useClerk, useUser } from "@clerk/react";
import { errorCode, orReverify } from "@shared/account/account";
import { ApiProblem } from "@shared/account/api";
import type {
  FoldCatch,
  FoldHint,
  FoldPreview,
  FoldWhy,
} from "@sikemux/protocol";
import { avatarSeed } from "@sikemux/protocol/avatar";
import { IconArrowLeft, IconInfo, IconMail } from "@shared/ui/Icons";
import { useEffect, useState } from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { Avatar, PersonAvatar } from "./Avatar.tsx";
import { DeleteAccount } from "./DeleteAccount.tsx";
import { useCheer } from "./Field.tsx";
import { Gate, GateActions, Loading } from "./Frame.tsx";
import { brandMark, iconMark } from "./marks.ts";
import { JOIN, RETURN_KEY, SIGNED_UP_FOR_KEY } from "./navigation.ts";
import { RestoreAccount } from "./RestoreAccount.tsx";
import { useReverified } from "./Reverify.tsx";
import { Btn, Problem, Txt } from "./ui.tsx";

const WAITING_KEY = "sikemux.fold";
const SIGNED_IN_HERE_KEY = "sikemux.signedInHere";
const settledKey = (userId: string) => `sikemux.oneAccount:${userId}`;

const maskEmail = (email: string) =>
  email.replace(/^(.)[^@]*(?=@)/, (_, first: string) => `${first}•••`);

/** A fold this tab started, waiting for the person's other account to confirm it. */
type Waiting = {
  id: string;
  secret: string;
  expiresAt: string;
  why: FoldWhy;
  waitingWords: string;
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

type FoldGate =
  | { kind: "checking" }
  | { kind: "open" }
  | { kind: "catch"; caught: FoldCatch; inviteToken?: string }
  | { kind: "confirm"; waiting: Waiting };

/**
 * Whether the signed-in account may go on to its pages: a new account may first be asked
 * whether the person already has one, and a fold this tab started is confirmed here.
 */
export function useFoldGate(
  signedIn: boolean,
  onDeleted: (deletesAt?: string) => void,
) {
  const { getToken, userId } = useAuth();
  const [gate, setGate] = useState<FoldGate>({ kind: "checking" });
  const [round, setRound] = useState(0);

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
  }, [signedIn, userId, getToken, round]);

  const open = () => {
    if (userId) localStorage.setItem(settledKey(userId), "1");
    sessionStorage.removeItem(SIGNED_UP_FOR_KEY);
    localStorage.setItem(SIGNED_IN_HERE_KEY, "1");
    setGate({ kind: "open" });
  };

  const screen =
    gate.kind === "catch" ? (
      <Catch
        caught={gate.caught}
        inviteToken={gate.inviteToken}
        onNew={open}
        onDeleted={onDeleted}
        onRestored={() => {
          setGate({ kind: "checking" });
          setRound((r) => r + 1);
        }}
      />
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
      <Loading />
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

  const start = async (why: FoldWhy, inviteToken?: string) => {
    setBusy(true);
    setProblem(undefined);
    try {
      const intent = await run(why, inviteToken);
      const waiting: Waiting = {
        id: intent.id,
        secret: intent.secret,
        expiresAt: intent.expiresAt,
        why,
        waitingWords: intent.waitingWords,
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
  onDeleted,
  onRestored,
}: {
  caught: FoldCatch;
  inviteToken?: string;
  onNew: () => void;
  onDeleted: (deletesAt?: string) => void;
  onRestored: () => void;
}) {
  const { getToken } = useAuth();
  const { signOut } = useClerk();
  const { start, busy, problem, confirm } = useStartFold();
  const [keeping, setKeeping] = useState(false);
  const [keepProblem, setKeepProblem] = useState<string>();
  const [deleting, setDeleting] = useState(false);
  const address = caught.address ? maskEmail(caught.address) : "this address";

  if (caught.deletesAt)
    return (
      <RestoreAccount deletesAt={caught.deletesAt} onRestored={onRestored} />
    );
  if (deleting)
    return (
      <DeleteAccount
        email={caught.address}
        restorableDays={undefined}
        load={{ state: "loaded", devices: [] }}
        onBack={() => setDeleting(false)}
        onDeleted={onDeleted}
      />
    );

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
    <Gate
      title={caught.title}
      lede={caught.words}
      mark={iconMark("mail")}
      scene="sign"
      tone={problem || keepProblem ? "failed" : "waiting"}
      say={
        caught.mustFold ? (
          <Txt bad disabled={busy} onClick={() => setDeleting(true)}>
            Delete this account instead
          </Txt>
        ) : undefined
      }
    >
      {confirm}
      <div className="gaddr">
        <IconMail size={15} />
        <span>{address}</span>
      </div>
      <Problem>{problem ?? keepProblem}</Problem>
      <GateActions>
        <Btn
          disabled={busy || keeping}
          busy={busy}
          onClick={() =>
            void start(why, why === "invite" ? inviteToken : undefined)
          }
        >
          Use my account
        </Btn>
        {caught.mustFold ? (
          <Btn
            quiet
            disabled={busy}
            onClick={() => void signOut({ redirectUrl: "/" })}
          >
            Sign out
          </Btn>
        ) : (
          <Btn
            quiet
            disabled={busy || keeping}
            busy={keeping}
            onClick={() => void keepNew()}
          >
            I&rsquo;m new here
          </Btn>
        )}
      </GateActions>
    </Gate>
  );
}

/** Above the sign-in, while a fold waits for the person's other account. */
export function FoldWaiting() {
  const [waiting, setWaiting] = useState(waitingFold);
  if (!waiting) return null;
  return (
    <div className="gnote">
      <IconInfo size={15} />
      <span>{waiting.waitingWords}</span>
      <Txt
        onClick={() => {
          dropWaiting();
          setWaiting(null);
        }}
      >
        Cancel
      </Txt>
    </div>
  );
}

type Read =
  | { state: "loading" }
  | { state: "failed"; problem: string; sameAccount: boolean }
  | { state: "loaded"; preview: FoldPreview }
  | { state: "done"; title: string; words: string };

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
  const { user } = useUser();
  const { signOut } = useClerk();
  const cheer = useCheer();
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
          sameAccount: errorCode(error) === "same_account",
        });
      }
    })();
    return () => {
      current = false;
    };
  }, [getToken, waiting]);

  if (read.state === "loading") return <Loading />;

  if (read.state === "failed")
    return (
      <Gate
        title="Nothing was brought over"
        lede={read.problem}
        mark={read.sameAccount ? iconMark("mail") : iconMark("link")}
        scene="sign"
        tone="failed"
        say={
          read.sameAccount
            ? undefined
            : "Start again from the account you want to bring over."
        }
      >
        <GateActions>
          {read.sameAccount ? (
            <>
              <Btn onClick={() => void signOut({ redirectUrl: "/" })}>
                Sign in with another account
              </Btn>
              <Btn quiet onClick={onCancel}>
                Cancel
              </Btn>
            </>
          ) : (
            <Btn onClick={onCancel}>Continue</Btn>
          )}
        </GateActions>
      </Gate>
    );

  const seatComing = waiting.why === "seat";

  if (read.state === "done")
    return (
      <Gate
        title={read.title}
        lede={read.words}
        mark={brandMark}
        scene="sign"
        tone="worked"
      >
        <GateActions>
          <Btn
            onClick={() =>
              seatComing ? void signOut({ redirectUrl: "/team" }) : onDone()
            }
          >
            {seatComing ? "Sign in" : "Continue"}
          </Btn>
        </GateActions>
      </Gate>
    );

  const { preview } = read;

  const bring = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await run();
      dropWaiting();
      cheer();
      setRead({
        state: "done",
        title: preview.doneTitle,
        words: preview.doneWords,
      });
    } catch (error) {
      setBusy(false);
      if (errorCode(error) === "reverification_cancelled") return;
      setProblem(problemText(error));
    }
  };

  const fromLabel = seatComing ? "This account" : "The new account";
  const toLabel = seatComing ? "Your other account" : "This account";
  const fromDetail = maskEmail(preview.address);
  const toDetail =
    preview.keptName ||
    (preview.keptAddress ? maskEmail(preview.keptAddress) : undefined) ||
    (!seatComing ? user?.fullName || user?.username || undefined : undefined) ||
    "Kept account";

  return (
    <Gate
      title={preview.title}
      lede={preview.words}
      mark={iconMark("users")}
      scene="sign"
      tone={problem ? "failed" : "waiting"}
    >
      {confirm}
      <div className="twos">
        <div className="twos-side">
          {seatComing ? (
            <Avatar size={32} />
          ) : (
            <PersonAvatar
              seed={avatarSeed(preview.address)}
              imageUrl={null}
              size={32}
            />
          )}
          <span className="twos-tx">
            <b>{fromLabel}</b>
            <small>{fromDetail}</small>
          </span>
        </div>
        <IconArrowLeft size={16} className="twos-arrow" aria-hidden="true" />
        <div className="twos-side">
          {seatComing ? (
            <PersonAvatar
              seed={avatarSeed(preview.keptAddress ?? preview.keptName ?? "to")}
              imageUrl={null}
              size={32}
            />
          ) : (
            <Avatar size={32} />
          )}
          <span className="twos-tx">
            <b>{toLabel}</b>
            <small>{toDetail}</small>
          </span>
        </div>
      </div>
      <Problem>{problem}</Problem>
      <GateActions>
        <Btn busy={busy} onClick={() => void bring()}>
          {seatComing ? "Move the seat" : "Bring it here"}
        </Btn>
        <Btn quiet disabled={busy} onClick={onCancel}>
          Cancel
        </Btn>
      </GateActions>
    </Gate>
  );
}

/** "Bring it here" where an org is started: a seat another of the person's accounts holds. */
export function BringSeat() {
  const { start, busy, problem, confirm } = useStartFold();
  return (
    <>
      {confirm}
      <Txt disabled={busy} onClick={() => void start("seat")}>
        Bring it here
      </Txt>
      <Problem>{problem}</Problem>
    </>
  );
}
