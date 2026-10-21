import { useReverification, useSession } from "@clerk/react";
import { chooseFactor } from "@sikemux/protocol/account";
import { useEffect, useRef, useState, type SubmitEvent } from "react";

import { problemText } from "./api.ts";
import { Panel } from "./Consent.tsx";
import { CodeEntry, PasswordField } from "./SignIn.tsx";

type Ask = {
  level: "first_factor" | "second_factor" | "multi_factor" | undefined;
  complete: () => void;
  cancel: () => void;
};

/**
 * Clerk's `useReverification`, with our own card in place of Clerk's prompt, which this app never
 * loads. `confirm` is the card while a recent sign-in is being asked for, to show instead of the page.
 */
export function useReverified<
  Fetcher extends Parameters<typeof useReverification>[0],
>(fetcher: Fetcher) {
  const [ask, setAsk] = useState<Ask>();
  const run = useReverification(fetcher, {
    onNeedsReverification: ({ level, complete, cancel }) =>
      setAsk({
        level,
        complete: () => {
          setAsk(undefined);
          complete();
        },
        cancel: () => {
          setAsk(undefined);
          cancel();
        },
      }),
  });
  return { run, confirm: ask ? <Reverify ask={ask} /> : null };
}

type Step =
  | { kind: "starting" }
  | { kind: "email"; email: string }
  | { kind: "password" }
  | { kind: "authenticator" }
  | { kind: "unsupported" };

function Reverify({ ask }: { ask: Ask }) {
  const { session } = useSession();
  const [step, setStep] = useState<Step>({ kind: "starting" });
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const started = useRef<Ask>(undefined);

  useEffect(() => {
    if (!session || started.current === ask) return;
    started.current = ask;
    void (async () => {
      try {
        const verification = await session.startVerification({
          level: ask.level ?? "first_factor",
        });
        if (verification.status === "complete") return ask.complete();
        if (verification.status === "needs_second_factor")
          return setStep(secondStep(verification.supportedSecondFactors));
        const factor = chooseFactor(verification.supportedFirstFactors ?? []);
        if (factor?.strategy === "email_code") {
          await session.prepareFirstFactorVerification({
            strategy: "email_code",
            emailAddressId: factor.emailAddressId,
          });
          setStep({ kind: "email", email: factor.to });
        } else {
          setStep(factor ? { kind: "password" } : { kind: "unsupported" });
        }
      } catch (error) {
        setProblem(problemText(error));
      }
    })();
  }, [session, ask]);

  const attempt = async (
    verify: () => ReturnType<NonNullable<typeof session>["startVerification"]>,
  ) => {
    setBusy(true);
    setProblem(undefined);
    try {
      const verification = await verify();
      if (verification.status === "complete") return ask.complete();
      setCode("");
      setStep(secondStep(verification.supportedSecondFactors));
    } catch (error) {
      setCode("");
      setProblem(problemText(error));
    } finally {
      setBusy(false);
    }
  };

  const typeCode = (typed: string) => {
    setCode(typed);
    if (typed.length !== 6 || !session) return;
    void attempt(() =>
      step.kind === "authenticator"
        ? session.attemptSecondFactorVerification({
            strategy: "totp",
            code: typed,
          })
        : session.attemptFirstFactorVerification({
            strategy: "email_code",
            code: typed,
          }),
    );
  };

  const submitPassword = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!session || !password) return;
    void attempt(() =>
      session.attemptFirstFactorVerification({
        strategy: "password",
        password,
      }),
    );
  };

  const cancel = (
    <button
      type="button"
      className="button"
      onClick={ask.cancel}
      disabled={busy}
    >
      Cancel
    </button>
  );

  return (
    <Panel>
      <h1>Confirm it&rsquo;s you</h1>
      {step.kind === "starting" ? (
        <p className="quiet">{problem ? "" : "One moment…"}</p>
      ) : null}
      {step.kind === "email" ? (
        <>
          <p className="lede">
            This needs a recent sign-in. Sikemux sent a code to{" "}
            <span className="ink">{step.email}</span>.
          </p>
          <CodeEntry
            code={code}
            onChange={typeCode}
            disabled={busy}
            failed={problem !== undefined && code.length === 0}
          />
        </>
      ) : null}
      {step.kind === "authenticator" ? (
        <>
          <p className="lede">Enter the code from your authenticator app.</p>
          <CodeEntry
            code={code}
            onChange={typeCode}
            disabled={busy}
            failed={problem !== undefined && code.length === 0}
          />
        </>
      ) : null}
      {step.kind === "password" ? (
        <form className="form" onSubmit={submitPassword}>
          <p className="lede">
            This needs a recent sign-in. Enter your password.
          </p>
          <PasswordField
            value={password}
            onChange={setPassword}
            autoComplete="current-password"
          />
          <button
            type="submit"
            className="button primary"
            disabled={busy || !password}
            data-busy={busy || undefined}
          >
            Confirm
          </button>
        </form>
      ) : null}
      {step.kind === "unsupported" ? (
        <p className="lede">
          Your account can&rsquo;t confirm here. Sign out and sign in again,
          then try once more.
        </p>
      ) : null}
      {problem ? <p className="problem">{problem}</p> : null}
      {cancel}
    </Panel>
  );
}

function secondStep(factors: { strategy: string }[] | null): Step {
  return factors?.some((factor) => factor.strategy === "totp")
    ? { kind: "authenticator" }
    : { kind: "unsupported" };
}
