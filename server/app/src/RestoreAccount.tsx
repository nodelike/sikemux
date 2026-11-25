import { useAuth, useClerk } from "@clerk/react";
import { useCallback, useEffect, useState } from "react";

import type { TeamAccess } from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import "./account.css";
import { api, problemText, signedInToken } from "./api.ts";
import { Foot } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { Btn, Hero, Problem } from "./ui.tsx";

type AccountState =
  | { state: "loading" }
  /** How long a deletion stays restorable is unknown when the account could not be read. */
  | { state: "active"; restorableDays?: number }
  | { state: "deleting"; deletesAt: string };

/** Whether the signed-in account is in use, or deleted and still restorable, read again whenever `version` moves. A failed read counts as in use, and the devices list says what went wrong. */
export function useAccountState(ready: boolean, version: number) {
  const { isSignedIn, getToken } = useAuth();
  const [account, setAccount] = useState<AccountState>({ state: "loading" });
  const [staff, setStaff] = useState(false);
  const [team, setTeam] = useState<TeamAccess>("shut");

  useEffect(() => {
    if (!ready || !isSignedIn) return;
    let current = true;
    void (async () => {
      try {
        const token = await signedInToken(getToken, "see your account");
        const read = await api(token).getAccount();
        if (!current) return;
        setStaff(read.staff);
        setTeam(read.team ?? "shut");
        setAccount(
          read.status === "deleting" && read.deletesAt
            ? { state: "deleting", deletesAt: read.deletesAt }
            : { state: "active", restorableDays: read.restorableDays },
        );
      } catch {
        if (current)
          setAccount((last) =>
            last.state === "loading" ? { state: "active" } : last,
          );
      }
    })();
    return () => {
      current = false;
    };
  }, [ready, isSignedIn, getToken, version]);

  const restored = useCallback(
    (restorableDays: number) => setAccount({ state: "active", restorableDays }),
    [],
  );
  return { account, staff, team, restored };
}

export function RestoreAccount({
  deletesAt,
  onRestored,
}: {
  deletesAt: string;
  onRestored: (restorableDays: number) => void;
}) {
  const { getToken } = useAuth();
  const { signOut } = useClerk();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const day = formatDay(deletesAt);

  const restore = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      const token = await signedInToken(getToken, "restore your account", {
        fresh: true,
      });
      const account = await api(token).restoreAccount();
      onRestored(account.restorableDays);
    } catch (error) {
      setBusy(false);
      setProblem(problemText(error));
    }
  };

  return (
    <Hero
      title="Your account is being deleted"
      lede={
        <>
          It will be deleted for good on <span className="lit">{day}</span>.
          Restore it to keep using Sikemux. Your devices were signed out, so
          each one signs in again.
        </>
      }
      mark={iconMark("warning")}
      scene="sign"
      tone="failed"
      foot={<Foot />}
    >
      <Problem>{problem}</Problem>
      <div className="acc-gate-do">
        <Btn onClick={() => void restore()} busy={busy}>
          Restore account
        </Btn>
        <Btn
          quiet
          disabled={busy}
          onClick={() => void signOut({ redirectUrl: "/" })}
        >
          Sign out
        </Btn>
      </div>
    </Hero>
  );
}
