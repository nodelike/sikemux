import release from "@sikemux/protocol/release.json";
import { useAuth } from "@clerk/react";
import {
  CONFIRM_WORD,
  confirmsDeletion,
  errorCode,
  orReverify,
} from "@shared/account/account";
import { formatDay } from "@shared/lib/format";
import {
  IconArrowLeft,
  IconClock,
  IconLaptop,
  IconPhone,
} from "@shared/ui/Icons";
import { useId, useState, type SubmitEvent } from "react";

import "./account.css";
import { api, problemText, signedInToken } from "./api.ts";
import type { Load } from "./Devices.tsx";
import { Foot } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { useReverified } from "./Reverify.tsx";
import { Btn, btnClass, Field, Hero, Problem, Txt } from "./ui.tsx";

const POLICY = "https://sikemux.com/delete-account";
const CONTACT = "contact@nodelike.com";

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Signed in: what deleting takes with it, and the one button that does it. */
export function DeleteAccount({
  restorableDays,
  load,
  onBack,
  onDeleted,
}: {
  email?: string;
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

  const devices = load.state === "loaded" ? load.devices : null;
  const hosts = devices?.filter((device) => device.role === "host") ?? [];
  const clients = devices?.filter((device) => device.role === "client") ?? [];

  return (
    <Hero
      title="Delete your account"
      lede={
        <>
          Changed your mind? Sign in again{" "}
          {restorableDays
            ? `within ${count(restorableDays, "day", "days")}`
            : "soon"}{" "}
          to restore it.
        </>
      }
      mark={iconMark("warning")}
      scene="sign"
      tone={busy ? "waiting" : "failed"}
      back={
        <Txt onClick={onBack}>
          <IconArrowLeft size={14} />
          Account
        </Txt>
      }
      foot={<Foot />}
    >
      {confirm}
      <ul className="acc-facts">
        <li>
          <IconLaptop size={16} />
          <span>
            <b>
              {devices
                ? `${count(hosts.length, "host", "hosts")} ${hosts.length === 1 ? "is" : "are"} signed out`
                : "Hosts are signed out"}
            </b>
            <small>
              The phones paired to them keep working.
              {hosts.length > 0
                ? ` ${hosts.map((device) => device.name).join(", ")}.`
                : ""}
            </small>
          </span>
        </li>
        <li>
          <IconPhone size={16} />
          <span>
            <b>
              {devices
                ? `${count(clients.length, "client", "clients")} ${clients.length === 1 ? "is" : "are"} signed out and removed from every host`
                : "Clients are signed out and removed from every host"}
            </b>
            {clients.length > 0 ? (
              <small>{clients.map((device) => device.name).join(", ")}.</small>
            ) : null}
          </span>
        </li>
        <li>
          <IconClock size={16} />
          <span>
            <b>
              {restorableDays
                ? `After ${count(restorableDays, "day", "days")}, everything is erased`
                : "Once it can no longer be restored, everything is erased"}
            </b>
            <small>
              Sign-in, device keys and history; backups within{" "}
              {release.account.backupDays} days after.
            </small>
          </span>
        </li>
      </ul>

      <form className="do" onSubmit={(e) => void submit(e)}>
        <label className="ask-for" htmlFor={fieldId}>
          Type <span className="mono lit">{CONFIRM_WORD}</span> to confirm
        </label>
        <Field
          id={fieldId}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={busy}
        />
        <Problem>{problem}</Problem>
        <Btn bad type="submit" disabled={!confirmsDeletion(typed)} busy={busy}>
          Delete account
        </Btn>
      </form>
      <p className="fine">
        You may be asked to confirm it&rsquo;s you first.{" "}
        <a href={POLICY}>How deletion works</a>
      </p>
    </Hero>
  );
}

/** Signed out: what this page is for, above the usual sign-in. */
export const DELETE_INTRO = {
  title: "Delete your Sikemux account",
  lede: (
    <>
      Sign in to delete it. Every device on the account is signed out, and
      phones are removed from every host. <a href={POLICY}>How it works</a>.
      Can&rsquo;t sign in? Email <a href={`mailto:${CONTACT}`}>{CONTACT}</a>.
    </>
  ),
};

export function AccountDeleted({ deletesAt }: { deletesAt?: string }) {
  return (
    <Hero
      title="Your account is deleted"
      lede={
        <>
          Every device on it is signed out. Changed your mind? Sign in again{" "}
          {deletesAt ? (
            <>
              before <span className="lit">{formatDay(deletesAt)}</span>
            </>
          ) : (
            "soon"
          )}{" "}
          to restore it. After that, your sign-in, device keys and the
          account&rsquo;s history are erased.
        </>
      }
      mark={iconMark("user")}
      scene="sign"
      foot={<Foot />}
    >
      <div className="acc-gate-do">
        <a className={btnClass({ quiet: true })} href="https://sikemux.com">
          Go to sikemux.com
        </a>
      </div>
      <p className="fine">
        Questions? Email <a href={`mailto:${CONTACT}`}>{CONTACT}</a>.
      </p>
    </Hero>
  );
}
