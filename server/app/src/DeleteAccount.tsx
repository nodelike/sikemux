import { useAuth } from "@clerk/react";
import {
  CONFIRM_WORD,
  confirmsDeletion,
  errorCode,
  orReverify,
} from "@sikemux/protocol/account";
import { useId, useState, type SubmitEvent } from "react";

import { api, formatDay, problemText, signedInToken } from "./api.ts";
import type { Load } from "./Devices.tsx";
import { BackIcon } from "./icons.tsx";
import { useReverified } from "./Reverify.tsx";
const POLICY = "https://sikemux.com/delete-account";
const CONTACT = "contact@nodelike.com";

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Signed in: what deleting takes with it, and the one button that does it. */
export function DeleteAccount({
  email,
  restorableDays,
  load,
  onBack,
  onDeleted,
}: {
  email: string | undefined;
  restorableDays: number | undefined;
  load: Load;
  onBack: () => void;
  onDeleted: (deletesAt: string) => void;
}) {
  const { getToken } = useAuth();
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const fieldId = useId();

  const { run: deleteAccount, confirm } = useReverified(async () => {
    const token = await signedInToken(getToken, "delete your account", {
      fresh: true,
    });
    return orReverify(() => api(token).deleteAccount());
  });

  const submit = async (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!confirmsDeletion(typed) || busy) return;
    setBusy(true);
    setProblem(undefined);
    try {
      const deletion = await deleteAccount();
      onDeleted(deletion.deletesAt);
    } catch (error) {
      setBusy(false);
      if (errorCode(error) === "reverification_cancelled") return;
      setProblem(problemText(error));
    }
  };

  if (confirm) return confirm;

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
        Sikemux. Changed your mind? Sign in again{" "}
        {restorableDays
          ? `within ${count(restorableDays, "day", "days")}`
          : "soon"}{" "}
        to restore it.
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
          {restorableDays
            ? `After ${count(restorableDays, "day", "days")},`
            : "Once it can no longer be restored,"}{" "}
          your sign-in, device keys and the account&rsquo;s history are erased,
          and backups within 30 days after that.
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
          disabled={!confirmsDeletion(typed) || busy}
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

export function AccountDeleted({ deletesAt }: { deletesAt?: string }) {
  return (
    <section className="panel">
      <h1>Your account is deleted</h1>
      <p>
        Every device on it is signed out. Changed your mind? Sign in again{" "}
        {deletesAt ? (
          <>
            before <span className="ink">{formatDay(deletesAt)}</span>
          </>
        ) : (
          "soon"
        )}{" "}
        to restore it. After that, your sign-in, device keys and the
        account&rsquo;s history are erased.
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
