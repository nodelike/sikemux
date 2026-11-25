import { useAuth } from "@clerk/react";
import { platformName } from "@shared/remote/access";
import type { PendingHost } from "@sikemux/protocol";
import { errorCode, orReverify } from "@shared/account/account";
import { useEffect, useRef, useState, type SubmitEvent } from "react";

import { formatDay } from "@shared/lib/format";
import {
  IconArrowLeft,
  IconClock,
  IconGlobe,
  IconServer,
} from "@shared/ui/Icons";
import { api, problemText, signedInToken } from "./api.ts";
import { useCheer } from "./Field.tsx";
import { Gate, GateActions, GateFacts } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { HOSTS } from "./navigation.ts";
import { useReverified } from "./Reverify.tsx";
import { Btn, btnClass, Problem, Txt } from "./ui.tsx";
import "./hosts.css";

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

function formatCode(raw: string): string {
  const clean = raw.toUpperCase().replace(/[^A-Z0-9]/g, "");
  return clean.length === 8
    ? `${clean.slice(0, 4)}-${clean.slice(4)}`
    : raw.toUpperCase();
}

/**
 * Where a host with no browser sends its person: they type the code it shows, check where and
 * when it asked, and add it to their account. A link never fills the code in, so a code someone
 * else started cannot be approved in one click.
 */
export function HostApproval() {
  const { getToken } = useAuth();
  const cheer = useCheer();
  const [typed, setTyped] = useState("");
  const [step, setStep] = useState<Step>({ kind: "enter" });
  const [busy, setBusy] = useState<"look" | "approve" | "refuse">();
  const [problem, setProblem] = useState<string>();
  const codeInput = useRef<HTMLInputElement>(null);
  const doneLink = useRef<HTMLAnchorElement>(null);

  useEffect(() => {
    if (step.kind === "enter") codeInput.current?.focus();
    else if (step.kind === "done") doneLink.current?.focus();
  }, [step.kind]);

  useEffect(() => {
    if (step.kind === "enter" && problem && !busy) codeInput.current?.focus();
  }, [step.kind, problem, busy]);

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

  if (step.kind === "done")
    return (
      <Gate
        key="done"
        title={step.approved ? `${step.name} is on your account` : "Refused"}
        lede={
          step.approved
            ? "Join it from Sikemux on your phone. The host still asks before it lets each phone in."
            : `${step.name} was not added. Its sign-in has stopped.`
        }
        mark={iconMark(step.approved ? "check" : "cross")}
        scene="sign"
        tone={step.approved ? "worked" : "failed"}
      >
        <GateActions>
          <a ref={doneLink} className={btnClass({ quiet: true })} href={HOSTS}>
            See your hosts
          </a>
        </GateActions>
      </Gate>
    );

  if (step.kind === "check") {
    const { code, host } = step;
    const where = [host.requestedNear, host.requestedFrom]
      .filter(Boolean)
      .join(", ");
    const alert = Boolean(problem || host.removedAt);
    return (
      <Gate
        key="check"
        title={`Add ${host.name} to your account?`}
        lede="Check that its screen shows this code."
        mark={iconMark("laptop")}
        scene={alert ? "sign" : "amber"}
        tone={alert ? "failed" : undefined}
        back={
          <Txt
            disabled={busy !== undefined}
            onClick={() => {
              setProblem(undefined);
              setStep({ kind: "enter" });
            }}
          >
            <IconArrowLeft size={14} />
            Back
          </Txt>
        }
        say={
          <>
            Only add a computer you are setting up yourself right now. If you
            did not just run <code>sikemux login</code> there, or it asked from
            somewhere you are not, someone else is trying to get into your
            account: choose Not mine.
          </>
        }
      >
        {confirm}
        <div className="hbig">{formatCode(code)}</div>
        <GateFacts
          items={[
            {
              key: "platform",
              icon: <IconServer size={16} />,
              title: `${platformName(host.platform)} · ${host.channel}`,
            },
            {
              key: "when",
              icon: <IconClock size={16} />,
              title: `Asked ${ago(host.requestedAt)}`,
            },
            {
              key: "where",
              icon: <IconGlobe size={16} />,
              title: `From ${where || "an address the server could not see"}`,
              detail: host.requestedNear ? (
                <>
                  Places from{" "}
                  <a href="https://db-ip.com" target="_blank" rel="noreferrer">
                    DB-IP
                  </a>
                </>
              ) : undefined,
            },
          ]}
        />
        <Problem>
          {host.removedAt
            ? `This host was removed from your account on ${formatDay(host.removedAt)}. Add it again only if you are setting it up yourself.`
            : null}
        </Problem>
        <Problem>{problem}</Problem>
        <GateActions>
          <Btn
            disabled={busy !== undefined}
            busy={busy === "approve"}
            onClick={() =>
              void run("approve", async () => {
                await approve(code);
                cheer();
                setStep({ kind: "done", name: host.name, approved: true });
              })
            }
          >
            Add host
          </Btn>
          <Btn
            quiet
            disabled={busy !== undefined}
            busy={busy === "refuse"}
            onClick={() =>
              void run("refuse", async () => {
                const token = await signedInToken(getToken, "refuse this host");
                await api(token).refuseHostSignIn(code);
                setStep({ kind: "done", name: host.name, approved: false });
              })
            }
          >
            Not mine
          </Btn>
        </GateActions>
      </Gate>
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
    <Gate
      key="enter"
      title="Add a host"
      lede={
        <>
          Enter the code that <code>sikemux login</code> shows on the computer.
        </>
      }
      mark={iconMark("laptop")}
      scene={busy === "look" ? "amber" : "sign"}
      tone={problem ? "failed" : undefined}
      back={
        <a className="txt gback" href={HOSTS}>
          <IconArrowLeft size={14} />
          Back
        </a>
      }
    >
      <form onSubmit={submit}>
        <label className={`hcode host-code${problem ? " bad" : ""}`}>
          <input
            ref={codeInput}
            value={typed}
            onChange={(event) => {
              setTyped(event.target.value);
              setProblem(undefined);
            }}
            placeholder="____-____"
            autoCapitalize="characters"
            autoComplete="off"
            spellCheck={false}
            autoFocus
            disabled={busy !== undefined}
            aria-label="Code"
          />
        </label>
        <Problem>{problem}</Problem>
        <GateActions>
          <Btn
            type="submit"
            disabled={!typed.trim() || busy !== undefined}
            busy={busy === "look"}
          >
            Continue
          </Btn>
        </GateActions>
      </form>
    </Gate>
  );
}
