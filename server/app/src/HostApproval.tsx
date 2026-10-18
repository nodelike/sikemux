import { useAuth } from "@clerk/react";
import type { PendingHost } from "@sikemux/protocol";
import { useEffect, useState, type FormEvent } from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { Panel } from "./Consent.tsx";

export const HOST = "/host";

const PLATFORMS: Record<string, string> = { macos: "macOS", linux: "Linux" };

type Step =
  | { kind: "enter" }
  | { kind: "check"; code: string; host: PendingHost }
  | { kind: "done"; name: string; approved: boolean };

/** Where a host with no browser sends its person: they check its code and add it to their account. */
export function HostApproval() {
  const { getToken } = useAuth();
  const given = new URLSearchParams(location.search).get("code") ?? "";
  const [typed, setTyped] = useState(given);
  const [step, setStep] = useState<Step>({ kind: "enter" });
  const [busy, setBusy] = useState<"look" | "approve" | "refuse" | undefined>(
    given ? "look" : undefined,
  );
  const [problem, setProblem] = useState<string>();

  const run = async (
    action: "look" | "approve" | "refuse",
    work: (token: string) => Promise<void>,
  ) => {
    setBusy(action);
    setProblem(undefined);
    try {
      await work(await signedInToken(getToken, "add this host"));
    } catch (error) {
      setProblem(problemText(error));
    } finally {
      setBusy(undefined);
    }
  };

  const show = (code: string) => async (token: string) => {
    setStep({ kind: "check", code, host: await api.pendingHost(token, code) });
  };

  useEffect(() => {
    if (!given) return;
    let current = true;
    void (async () => {
      try {
        const token = await signedInToken(getToken, "add this host");
        const host = await api.pendingHost(token, given);
        if (current) setStep({ kind: "check", code: given, host });
      } catch (error) {
        if (current) setProblem(problemText(error));
      } finally {
        if (current) setBusy(undefined);
      }
    })();
    return () => {
      current = false;
    };
  }, [given, getToken]);

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
    return (
      <Panel>
        <h1>Add {host.name} to your account?</h1>
        <p className="lede">
          {PLATFORMS[host.platform] ?? host.platform} · {host.channel}. Only add
          a computer you are setting up yourself right now: phones on your
          account will find it and can ask to join it.
        </p>
        {problem ? <p className="problem">{problem}</p> : null}
        <div className="consent-actions">
          <button
            type="button"
            className="button"
            disabled={busy !== undefined}
            data-busy={busy === "refuse" || undefined}
            onClick={() =>
              void run("refuse", async (token) => {
                await api.refuseHost(token, code);
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
              void run("approve", async (token) => {
                await api.approveHost(token, code);
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

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const code = typed.trim();
    if (code) void run("look", show(code));
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
