import { useAuth, useReverification } from "@clerk/react";
import { useId, useState, type SubmitEvent } from "react";

import { api, ApiProblem } from "./api.ts";
import { errorCode } from "./clerkErrors.ts";
import type { Load } from "./Devices.tsx";
import { BackIcon } from "./icons.tsx";

const CONFIRM_WORD = "delete";
const POLICY = "https://sikemux.com/delete-account";
const CONTACT = "contact@nodelike.com";

/** What `useReverification` takes as "ask the person to sign in again": the API wants a first factor from the last 10 minutes. */
const NEEDS_REVERIFICATION = {
  clerk_error: {
    type: "forbidden",
    reason: "reverification-error",
    metadata: {
      reverification: { level: "first_factor", afterMinutes: 10 },
    },
  },
};

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Signed in: what deleting takes with it, and the one button that does it. */
export function DeleteAccount({
  email,
  load,
  onBack,
  onDeleted,
}: {
  email: string | undefined;
  load: Load;
  onBack: () => void;
  onDeleted: () => void;
}) {
  const { getToken } = useAuth();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const fieldId = useId();

  const deleteAccount = useReverification(async () => {
    const token = await getToken({ skipCache: true });
    if (!token) throw new Error("Sign in again to delete your account.");
    try {
      return await api.deleteAccount(token);
    } catch (error) {
      if (error instanceof ApiProblem && error.code === "reverify_required")
        return NEEDS_REVERIFICATION;
      throw error;
    }
  });

  const submit = async (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (typed.trim().toLowerCase() !== CONFIRM_WORD || busy) return;
    setBusy(true);
    setProblem(undefined);
    try {
      await deleteAccount();
      onDeleted();
    } catch (error) {
      setBusy(false);
      if (errorCode(error) === "reverification_cancelled") return;
      setProblem(
        error instanceof Error
          ? error.message
          : "Something went wrong; try again.",
      );
    }
  };

  const devices = load.state === "loaded" ? load.devices : null;
  const hosts = devices?.filter((device) => device.role === "host") ?? [];
  const clients = devices?.filter((device) => device.role === "client") ?? [];

  return (
    <div className="deletion">
      <button type="button" className="back" onClick={onBack}>
        <BackIcon />
        Your devices
      </button>
      <h1>Delete your account</h1>
      <p className="lede">
        This deletes {email ? <span className="ink">{email}</span> : "it"} from
        Sikemux. Changed your mind? Sign in again within 30 days to restore it.
      </p>

      <ul className="facts">
        <li>
          <span className="ink">Hosts</span>
          {devices ? ` (${hosts.length})` : ""} are signed out. The phones
          paired to them keep working.
        </li>
        <li>
          <span className="ink">Clients</span>
          {devices ? ` (${clients.length})` : ""} are signed out and removed
          from every host.
        </li>
        <li>
          After 30 days your sign-in, device keys and the account&rsquo;s
          history are erased, and backups within 30 days after that.
        </li>
      </ul>
      {devices && devices.length > 0 ? (
        <p className="quiet small">
          {count(devices.length, "device", "devices")}:{" "}
          {devices.map((device) => device.name).join(", ")}.
        </p>
      ) : null}

      <form className="form confirm-delete" onSubmit={(e) => void submit(e)}>
        <label htmlFor={fieldId}>
          Type <span className="word">{CONFIRM_WORD}</span> to confirm
        </label>
        <input
          id={fieldId}
          className="field"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={busy}
        />
        {problem ? (
          <p className="problem" role="alert">
            {problem}
          </p>
        ) : null}
        <button
          type="submit"
          className="button danger"
          disabled={typed.trim().toLowerCase() !== CONFIRM_WORD || busy}
          data-busy={busy || undefined}
        >
          Delete account
        </button>
        <p className="quiet small">
          You may be asked to confirm it's you first.{" "}
          <a href={POLICY}>How deletion works</a>
        </p>
      </form>
    </div>
  );
}

/** Signed out: what this page is for, above the usual sign-in. */
export function DeleteAccountIntro() {
  return (
    <section className="intro">
      <h1>Delete your Sikemux account</h1>
      <p>
        Sign in to delete it. Every device on the account is signed out, and
        phones are removed from every host. <a href={POLICY}>How it works</a>
      </p>
      <p className="quiet small">
        Can't sign in? Email <a href={`mailto:${CONTACT}`}>{CONTACT}</a>.
      </p>
    </section>
  );
}

export function AccountDeleted() {
  return (
    <section className="panel">
      <h1>Your account is deleted</h1>
      <p>
        Every device on it is signed out. Changed your mind? Sign in again
        within 30 days to restore it. After that, your sign-in, device keys and
        the account&rsquo;s history are erased.
      </p>
      <p className="quiet small">
        Questions? Email <a href={`mailto:${CONTACT}`}>{CONTACT}</a>.
      </p>
      <a className="button" href="https://sikemux.com">
        Go to sikemux.com
      </a>
    </section>
  );
}
