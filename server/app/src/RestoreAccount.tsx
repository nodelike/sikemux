import { useAuth } from "@clerk/react";
import { useCallback, useEffect, useState } from "react";

import { api } from "./api.ts";

export type AccountState =
  | { state: "loading" }
  | { state: "active" }
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
        const token = await getToken();
        if (!token) throw new Error("signed out");
        const read = await api.account(token);
        if (!current) return;
        setAccount(
          read.status === "deleting" && read.deletesAt
            ? { state: "deleting", deletesAt: read.deletesAt }
            : { state: "active" },
        );
      } catch {
        if (current) setAccount({ state: "active" });
      }
    })();
    return () => {
      current = false;
    };
  }, [ready, isSignedIn, getToken]);

  const restored = useCallback(() => setAccount({ state: "active" }), []);
  return { account, restored };
}

export function RestoreAccount({
  deletesAt,
  onRestored,
}: {
  deletesAt: string;
  onRestored: () => void;
}) {
  const { getToken } = useAuth();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const day = new Date(deletesAt).toLocaleDateString(undefined, {
    day: "numeric",
    month: "long",
    year: "numeric",
  });

  const restore = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      const token = await getToken({ skipCache: true });
      if (!token) throw new Error("Sign in again to restore your account.");
      await api.restoreAccount(token);
      onRestored();
    } catch (error) {
      setBusy(false);
      setProblem(
        error instanceof Error
          ? error.message
          : "Something went wrong; try again.",
      );
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
