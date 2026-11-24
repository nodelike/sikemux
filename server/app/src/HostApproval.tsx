import { useAuth } from "@clerk/react";
import { platformName } from "@shared/remote/access";
import type { PendingHost } from "@sikemux/protocol";
import { errorCode, orReverify } from "@shared/account/account";
import { useState, type SubmitEvent } from "react";

import { formatDay } from "@shared/lib/format";
import { IconBack } from "@shared/ui/Icons";
import { api, problemText, signedInToken } from "./api.ts";
import { Foot } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { HOSTS } from "./navigation.ts";
import { useReverified } from "./Reverify.tsx";
import { Btn, btnClass, Field, Hero, Problem, Txt } from "./ui.tsx";

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

  if (step.kind === "done")
    return (
      <Hero
        title={step.approved ? `${step.name} is on your account` : "Refused"}
        lede={
          step.approved
            ? "Join it from Sikemux on your phone. The host still asks before it lets each phone in."
            : `${step.name} was not added. Its sign-in has stopped.`
        }
        mark={iconMark("server")}
        scene="teal"
        tone={step.approved ? "worked" : undefined}
        foot={<Foot />}
      >
        <div className="do">
          <a className={btnClass({ quiet: true })} href={HOSTS}>
            See your hosts
          </a>
        </div>
      </Hero>
    );

  if (step.kind === "check") {
    const { code, host } = step;
    const where = [host.requestedNear, host.requestedFrom]
      .filter(Boolean)
      .join(", ");
    return (
      <Hero
        title={`Add ${host.name} to your account?`}
        lede="Check that its screen shows this code."
        mark={iconMark("server")}
        scene="teal"
        tone={
          problem || host.removedAt ? "failed" : busy ? "waiting" : undefined
        }
        back={
          <Txt
            disabled={busy !== undefined}
            onClick={() => {
              setProblem(undefined);
              setStep({ kind: "enter" });
            }}
          >
            <IconBack className="ic" />
            Back
          </Txt>
        }
        foot={<Foot />}
      >
        {confirm}
        <p className="big-code">{code.toUpperCase()}</p>
        <ul className="facts">
          <li>
            {platformName(host.platform)} · {host.channel}
          </li>
          <li>Asked {ago(host.requestedAt)}</li>
          <li>
            From {where || "an address the server could not see"}
            {host.requestedNear ? (
              <>
                {" "}
                (places from{" "}
                <a href="https://db-ip.com" target="_blank" rel="noreferrer">
                  DB-IP
                </a>
                )
              </>
            ) : null}
          </li>
        </ul>
        <p className="fine">
          Only add a computer you are setting up yourself right now. If you did
          not just run <span className="mono lit">sikemux login</span> there, or
          it asked from somewhere you are not, someone else is trying to get
          into your account: choose Not mine.
        </p>
        <div className="do">
          <Problem>
            {host.removedAt
              ? `This host was removed from your account on ${formatDay(host.removedAt)}. Add it again only if you are setting it up yourself.`
              : null}
          </Problem>
          <Problem>{problem}</Problem>
          <Btn
            disabled={busy !== undefined}
            busy={busy === "approve"}
            onClick={() =>
              void run("approve", async () => {
                await approve(code);
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
        </div>
      </Hero>
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
    <Hero
      title="Add a host"
      lede={
        <>
          Enter the code that <span className="mono lit">sikemux login</span>{" "}
          shows on the computer.
        </>
      }
      mark={iconMark("server")}
      scene="teal"
      tone={problem ? "failed" : busy ? "waiting" : undefined}
      foot={<Foot />}
    >
      <form className="do" onSubmit={submit}>
        <Field
          className="field code-field"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          placeholder="BCDF-GHJK"
          autoCapitalize="characters"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          aria-label="Code"
        />
        <Problem>{problem}</Problem>
        <Btn type="submit" disabled={!typed.trim()} busy={busy === "look"}>
          Continue
        </Btn>
      </form>
    </Hero>
  );
}
