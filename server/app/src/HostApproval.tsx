import { useAuth } from "@clerk/react";
import { platformName } from "@shared/remote/access";
import type { PendingHost } from "@sikemux/protocol";
import { errorCode, orReverify } from "@sikemux/protocol/account";
import { useState, type SubmitEvent } from "react";

import { api, formatDay, problemText, signedInToken } from "./api.ts";
import { Panel } from "./Consent.tsx";
import { useReverified } from "./Reverify.tsx";

type Step =
  | { kind: "enter" }
  | { kind: "check"; code: string; host: PendingHost }
  | { kind: "done"; name: string; approved: boolean };

function ago(iso: string): string {
  const minutes = Math.round((Date.parse(iso) - Date.now()) / 60_000);
  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(
    Math.min(0, minutes),
    "minute",
  );
}

/**
 * Where a host with no browser sends its person: they type the code it shows, check where and
 * when it asked, and add it to their account. A link never fills the code in, so a code someone
 * else started cannot be approved in one click.
 */
export function HostApproval() {
  const { getToken } = useAuth();
  const [typed, setTyped] = useState("");
  const [step, setStep] = useState<Step>({ kind: "enter" });
  const [busy, setBusy] = useState<"look" | "approve" | "refuse">();
  const [problem, setProblem] = useState<string>();

  const { run: approve, confirm } = useReverified(async (code: string) => {
    const token = await signedInToken(getToken, "add this host", {
      fresh: true,
    });
    return orReverify(() => api(token).approveHostSignIn(code));
  });

  const run = async (
    action: "look" | "approve" | "refuse",
    work: () => Promise<void>,
  ) => {
    setBusy(action);
    setProblem(undefined);
    try {
      await work();
    } catch (error) {
      if (errorCode(error) !== "reverification_cancelled")
        setProblem(problemText(error));
    } finally {
      setBusy(undefined);
    }
  };

  if (confirm) return confirm;

  if (step.kind === "done")
    return (
      <Panel>
        <h1>{step.approved ? `${step.name} is on your account` : "Refused"}</h1>
        <p className="lede">
          {step.approved
            ? "Join it from Sikemux on your phone. The host still asks before it lets each phone in."
            : `${step.name} was not added. Its sign-in has stopped.`}
        </p>
      </Panel>
    );

  if (step.kind === "check") {
    const { code, host } = step;
    const where = [host.requestedNear, host.requestedFrom]
      .filter(Boolean)
      .join(", ");
    return (
      <Panel>
        <h1>Add {host.name} to your account?</h1>
        <ul className="consent-scopes">
          <li>
            {platformName(host.platform)} · {host.channel}
          </li>
          <li>Asked {ago(host.requestedAt)}</li>
          <li>From {where || "an address the server could not see"}</li>
        </ul>
        {host.removedAt ? (
          <p className="problem">
            This host was removed from your account on{" "}
            {formatDay(host.removedAt)}. Add it again only if you are setting it
            up yourself.
          </p>
        ) : null}
        <p className="lede">
          Only add a computer you are setting up yourself right now. If you did
          not just run <span className="ink">sikemux login</span> there, or it
          asked from somewhere you are not, someone else is trying to get into
          your account: choose Not mine.
        </p>
        {host.requestedNear ? (
          <p className="quiet small">
            Places from{" "}
            <a href="https://db-ip.com" target="_blank" rel="noreferrer">
              DB-IP
            </a>
            .
          </p>
        ) : null}
        {problem ? <p className="problem">{problem}</p> : null}
        <div className="consent-actions">
          <button
            type="button"
            className="button"
            disabled={busy !== undefined}
            data-busy={busy === "refuse" || undefined}
            onClick={() =>
              void run("refuse", async () => {
                const token = await signedInToken(getToken, "refuse this host");
                await api(token).refuseHostSignIn(code);
                setStep({ kind: "done", name: host.name, approved: false });
              })
            }
          >
            Not mine
          </button>
          <button
            type="button"
            className="button primary"
            disabled={busy !== undefined}
            data-busy={busy === "approve" || undefined}
            onClick={() =>
              void run("approve", async () => {
                await approve(code);
                setStep({ kind: "done", name: host.name, approved: true });
              })
            }
          >
            Add host
          </button>
        </div>
      </Panel>
    );
  }

  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    const code = typed.trim();
    if (!code) return;
    void run("look", async () => {
      const token = await signedInToken(getToken, "add this host");
      setStep({
        kind: "check",
        code,
        host: await api(token).getPendingHost(code),
      });
    });
  };
  return (
    <Panel>
      <h1>Add a host</h1>
      <p className="lede">
        Enter the code that <span className="ink">sikemux login</span> shows on
        the computer.
      </p>
      <form className="form" onSubmit={submit}>
        <input
          className="field"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          placeholder="BCDF-GHJK"
          autoCapitalize="characters"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          aria-label="Code"
        />
        {problem ? <p className="problem">{problem}</p> : null}
        <button
          type="submit"
          className="button primary"
          disabled={busy !== undefined || !typed.trim()}
          data-busy={busy === "look" || undefined}
        >
          Continue
        </button>
      </form>
    </Panel>
  );
}
