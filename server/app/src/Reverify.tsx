import { useClerk, useReverification, useSession } from "@clerk/react";
import { chooseFactor } from "@shared/account/account";
import { useEffect, useRef, useState } from "react";

import "./account.css";
import { problemText } from "./api.ts";
import { CodeEntry, PasswordField } from "./SignIn.tsx";
import { Btn, Dialog, Problem } from "./ui.tsx";

type Ask = {
  level: "first_factor" | "second_factor" | "multi_factor" | undefined;
  complete: () => void;
  cancel: () => void;
};

/**
 * Clerk's `useReverification`, with our own dialog in place of Clerk's prompt, which this app never
 * loads. `confirm` is the dialog while a recent sign-in is being asked for, to show over the page.
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
  const { signOut } = useClerk();
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

  const submitPassword = () => {
    if (!session || !password) return;
    void attempt(() =>
      session.attemptFirstFactorVerification({
        strategy: "password",
        password,
      }),
    );
  };

  const digits = (
    <CodeEntry
      code={code}
      onChange={typeCode}
      disabled={busy}
      failed={problem !== undefined && code.length === 0}
    />
  );

  return (
    <Dialog
      title="Confirm it's you"
      busy={busy}
      onClose={ask.cancel}
      onSubmit={step.kind === "password" ? submitPassword : undefined}
      foot={
        <>
          <Btn quiet onClick={ask.cancel} disabled={busy}>
            Cancel
          </Btn>
          {step.kind === "password" ? (
            <Btn type="submit" disabled={!password} busy={busy}>
              Confirm
            </Btn>
          ) : step.kind === "unsupported" ? (
            <Btn
              disabled={busy}
              onClick={() => void signOut({ redirectUrl: "/" })}
            >
              Sign out
            </Btn>
          ) : null}
        </>
      }
    >
      {step.kind === "starting" && !problem ? (
        <p className="rv-busy">
          <span className="rv-spin" aria-hidden="true" />
          One moment…
        </p>
      ) : null}
      {step.kind === "email" ? (
        <>
          <p>
            This needs a recent sign-in. Sikemux sent a code to{" "}
            <span className="lit">{step.email}</span>.
          </p>
          {digits}
        </>
      ) : null}
      {step.kind === "authenticator" ? (
        <>
          <p>Enter the code from your authenticator app.</p>
          {digits}
        </>
      ) : null}
      {step.kind === "password" ? (
        <>
          <p>This needs a recent sign-in. Enter your password.</p>
          <PasswordField
            value={password}
            onChange={setPassword}
            autoComplete="current-password"
          />
        </>
      ) : null}
      {step.kind === "unsupported" ? (
        <p>
          Your account can&rsquo;t confirm here. Sign out and sign in again,
          then try once more.
        </p>
      ) : null}
      <Problem>{problem}</Problem>
    </Dialog>
  );
}

function secondStep(factors: { strategy: string }[] | null): Step {
  return factors?.some((factor) => factor.strategy === "totp")
    ? { kind: "authenticator" }
    : { kind: "unsupported" };
}
