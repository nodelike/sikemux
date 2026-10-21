import { useAuth } from "@clerk/react";
import { useCallback, useEffect, useState } from "react";

import { api, formatDay, problemText, signedInToken } from "./api.ts";

export type AccountState =
  | { state: "loading" }
  /** How long a deletion stays restorable is unknown when the account could not be read. */
  | { state: "active"; restorableDays?: number }
  | { state: "deleting"; deletesAt: string };

/** Whether the signed-in account is in use, or deleted and still restorable. A failed read counts as in use, and the devices list says what went wrong. */
export function useAccountState(ready: boolean) {
  const { isSignedIn, getToken } = useAuth();
  const [account, setAccount] = useState<AccountState>({ state: "loading" });

  useEffect(() => {
    if (!ready || !isSignedIn) return;
    let current = true;
    void (async () => {
      try {
        const token = await signedInToken(getToken, "see your account");
        const read = await api(token).getAccount();
        if (!current) return;
        setAccount(
          read.status === "deleting" && read.deletesAt
            ? { state: "deleting", deletesAt: read.deletesAt }
            : { state: "active", restorableDays: read.restorableDays },
        );
      } catch {
        if (current) setAccount({ state: "active" });
      }
    })();
    return () => {
      current = false;
    };
  }, [ready, isSignedIn, getToken]);

  const restored = useCallback(
    (restorableDays: number) => setAccount({ state: "active", restorableDays }),
    [],
  );
  return { account, restored };
}

export function RestoreAccount({
  deletesAt,
  onRestored,
}: {
  deletesAt: string;
  onRestored: (restorableDays: number) => void;
}) {
  const { getToken } = useAuth();
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
    <div className="deletion">
      <h1>Your account is being deleted</h1>
      <p className="lede">
        It will be deleted for good on <span className="ink">{day}</span>.
        Restore it to keep using Sikemux. Your devices were signed out, so each
        one signs in again.
      </p>
      {problem ? (
        <p className="problem" role="alert">
          {problem}
        </p>
      ) : null}
      <button
        type="button"
        className="button"
        onClick={() => void restore()}
        disabled={busy}
        data-busy={busy || undefined}
      >
        Restore account
      </button>
    </div>
  );
}
