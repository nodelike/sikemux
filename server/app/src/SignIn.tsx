import { useSignIn, useSignUp } from "@clerk/react";
import { errorCode, explain } from "@sikemux/protocol/account";
import { useEffect, useRef, useState, type SubmitEvent } from "react";

import {
  AppleMark,
  BackIcon,
  EyeIcon,
  GitHubMark,
  GoogleMark,
  Logo,
} from "./icons.tsx";
import { HOST, isSharePath, rememberReturn } from "./navigation.ts";

/** Why an emailed code is asked for: a browser new to the account, a new account, or a new password. */
type CodePurpose = "trust" | "verify" | "reset";

type Step =
  | { kind: "start" }
  | { kind: "password" }
  | { kind: "create" }
  | { kind: "code"; purpose: CodePurpose }
  | { kind: "new-password" };

type Provider = "oauth_apple" | "oauth_google" | "oauth_github";

type Action = "apple" | "google" | "github" | "submit";

const ACTIONS: Record<Provider, Action> = {
  oauth_apple: "apple",
  oauth_google: "google",
  oauth_github: "github",
};

const CODE_DETAIL: Record<CodePurpose, string> = {
  trust: "to confirm this browser is yours",
  verify: "to confirm the address is yours",
  reset: "so you can choose a new password",
};

/** On a shared chat's page a finished sign-in stays there, where the chat then opens, instead of going to the account pages. */
function staying() {
  return isSharePath(location.pathname) || location.pathname === HOST
    ? { navigate: () => undefined }
    : undefined;
}

export function SignIn({ ready }: { ready: boolean }) {
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const [step, setStep] = useState<Step>({ kind: "start" });
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState<Action>();
  const [problem, setProblem] = useState<string>();
  const queued = useRef<{ action: Action; work: () => Promise<void> }>(null);

  const go = (next: Step) => {
    setProblem(undefined);
    setPassword("");
    setCode("");
    setStep(next);
  };

  const run = async (action: Action, work: () => Promise<void>) => {
    setBusy(action);
    setProblem(undefined);
    try {
      await work();
    } catch (error) {
      setProblem(explain(error));
    } finally {
      setBusy(undefined);
    }
  };

  /** Runs now, or as soon as Clerk has loaded, so a click in the first moment is not lost. */
  const attempt = (action: Action, work: () => Promise<void>) => {
    if (ready) return void run(action, work);
    queued.current = { action, work };
    setBusy(action);
  };

  useEffect(() => {
    if (!ready || !queued.current) return;
    const { action, work } = queued.current;
    queued.current = null;
    void run(action, work);
  }, [ready]);

  const check = (result: { error: unknown }) => {
    if (result.error) throw result.error;
  };

  const afterPassword = async () => {
    if (signIn.status === "complete")
      return check(await signIn.finalize(staying()));
    if (
      signIn.status === "needs_client_trust" ||
      signIn.status === "needs_second_factor"
    ) {
      check(await signIn.mfa.sendEmailCode());
      return go({ kind: "code", purpose: "trust" });
    }
    throw new Error(
      "This account needs a sign-in step this page does not do yet.",
    );
  };

  const withProvider = (strategy: Provider) =>
    attempt(ACTIONS[strategy], async () => {
      rememberReturn();
      check(
        await signIn.sso({
          strategy,
          redirectUrl: new URL("/", location.href).href,
          redirectCallbackUrl: new URL("/sso-callback", location.href).href,
        }),
      );
    });

  const submitEmail = () =>
    attempt("submit", async () => {
      const { error } = await signIn.create({ identifier: email.trim() });
      if (errorCode(error) === "form_identifier_not_found")
        return go({ kind: "create" });
      check({ error });
      go({ kind: "password" });
    });

  const submitPassword = () =>
    attempt("submit", async () => {
      check(await signIn.password({ password }));
      await afterPassword();
    });

  const submitNewAccount = () =>
    attempt("submit", async () => {
      check(await signUp.password({ emailAddress: email.trim(), password }));
      check(await signUp.verifications.sendEmailCode());
      go({ kind: "code", purpose: "verify" });
    });

  const submitNewPassword = () =>
    attempt("submit", async () => {
      check(await signIn.resetPasswordEmailCode.submitPassword({ password }));
      await afterPassword();
    });

  const forgot = () =>
    attempt("submit", async () => {
      check(await signIn.resetPasswordEmailCode.sendCode());
      go({ kind: "code", purpose: "reset" });
    });

  const submitCode = (purpose: CodePurpose, typed: string) =>
    attempt("submit", async () => {
      try {
        if (purpose === "trust") {
          check(await signIn.mfa.verifyEmailCode({ code: typed }));
          return check(await signIn.finalize(staying()));
        }
        if (purpose === "reset") {
          check(
            await signIn.resetPasswordEmailCode.verifyCode({ code: typed }),
          );
          return go({ kind: "new-password" });
        }
        check(await signUp.verifications.verifyEmailCode({ code: typed }));
        if (signUp.status !== "complete")
          throw new Error(
            "The account needs more than this page asks for; try again.",
          );
        check(await signUp.finalize(staying()));
      } catch (error) {
        setCode("");
        throw error;
      }
    });

  const typeCode = (typed: string) => {
    setCode(typed);
    if (typed.length === 6 && step.kind === "code")
      submitCode(step.purpose, typed);
  };

  const back = () => {
    void signIn.reset();
    go({ kind: "start" });
  };

  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    if (step.kind === "start") return submitEmail();
    if (step.kind === "password") return submitPassword();
    if (step.kind === "create") return submitNewAccount();
    if (step.kind === "new-password") return submitNewPassword();
  };

  if (step.kind === "start")
    return (
      <section className="panel">
        <div className="brand">
          <Logo size={22} />
          <span>Sikemux</span>
        </div>
        <h1>Sign in to Sikemux</h1>
        <p className="lede">
          One account for every device you use with Sikemux. Sign in on each,
          and they find each other.
        </p>
        <div className="providers">
          <button
            type="button"
            className="button"
            onClick={() => withProvider("oauth_apple")}
            disabled={busy !== undefined}
            data-busy={busy === "apple" || undefined}
          >
            <AppleMark />
            Continue with Apple
          </button>
          <button
            type="button"
            className="button"
            onClick={() => withProvider("oauth_google")}
            disabled={busy !== undefined}
            data-busy={busy === "google" || undefined}
          >
            <GoogleMark />
            Continue with Google
          </button>
          <button
            type="button"
            className="button"
            onClick={() => withProvider("oauth_github")}
            disabled={busy !== undefined}
            data-busy={busy === "github" || undefined}
          >
            <GitHubMark />
            Continue with GitHub
          </button>
        </div>
        <div className="divider" role="separator">
          <span>or</span>
        </div>
        <form className="form" onSubmit={submit}>
          <input
            className="field"
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            placeholder="you@example.com"
            autoComplete="email"
            aria-label="Email"
            required
          />
          {problem ? <p className="problem">{problem}</p> : null}
          <button
            type="submit"
            className="button primary"
            disabled={busy !== undefined}
            data-busy={busy === "submit" || undefined}
          >
            Continue with email
          </button>
        </form>
      </section>
    );

  if (step.kind === "code")
    return (
      <section className="panel">
        <BackLink onClick={back} />
        <h1>Check your email</h1>
        <p className="lede">
          Sikemux sent a code to <span className="ink">{email}</span>,{" "}
          {CODE_DETAIL[step.purpose]}.
        </p>
        <CodeEntry
          code={code}
          onChange={typeCode}
          disabled={busy !== undefined}
          failed={problem !== undefined && code.length === 0}
        />
        {problem ? <p className="problem">{problem}</p> : null}
      </section>
    );

  const title =
    step.kind === "password"
      ? "Enter your password"
      : step.kind === "create"
        ? "Choose a password"
        : "Choose a new password";
  const action =
    step.kind === "password"
      ? "Sign in"
      : step.kind === "create"
        ? "Create account"
        : "Save password";

  return (
    <section className="panel">
      <BackLink onClick={back} />
      <h1>{title}</h1>
      <p className="lede">
        {step.kind === "create" ? (
          <>
            A new account for <span className="ink">{email}</span>. At least 15
            characters; a password found in a known breach is refused.
          </>
        ) : (
          <span className="ink">{email}</span>
        )}
      </p>
      <form className="form" onSubmit={submit}>
        <input
          type="email"
          value={email}
          autoComplete="username"
          readOnly
          hidden
        />
        <PasswordField
          key={step.kind}
          value={password}
          onChange={setPassword}
          autoComplete={
            step.kind === "password" ? "current-password" : "new-password"
          }
        />
        {step.kind === "create" ? <div id="clerk-captcha" /> : null}
        {problem ? <p className="problem">{problem}</p> : null}
        <button
          type="submit"
          className="button primary"
          disabled={busy !== undefined}
          data-busy={busy === "submit" || undefined}
        >
          {action}
        </button>
        {step.kind === "password" ? (
          <button
            type="button"
            className="link"
            onClick={forgot}
            disabled={busy !== undefined}
          >
            Forgot your password?
          </button>
        ) : null}
      </form>
    </section>
  );
}

function BackLink({ onClick }: { onClick: () => void }) {
  return (
    <button type="button" className="back" onClick={onClick}>
      <BackIcon />
      Back
    </button>
  );
}

export function PasswordField({
  value,
  onChange,
  autoComplete,
}: {
  value: string;
  onChange: (value: string) => void;
  autoComplete: string;
}) {
  const [shown, setShown] = useState(false);
  return (
    <div className="password">
      <input
        className="field"
        type={shown ? "text" : "password"}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder="Password"
        autoComplete={autoComplete}
        aria-label="Password"
        autoFocus
        required
      />
      <button
        type="button"
        className="reveal"
        onClick={() => setShown((was) => !was)}
        aria-label={shown ? "Hide password" : "Show password"}
      >
        <EyeIcon crossed={shown} />
      </button>
    </div>
  );
}

/** Six tiles under one real input, so typing, pasting and the browser's code autofill all work. */
export function CodeEntry({
  code,
  onChange,
  disabled,
  failed,
}: {
  code: string;
  onChange: (code: string) => void;
  disabled: boolean;
  failed: boolean;
}) {
  const [focused, setFocused] = useState(true);
  return (
    <label className="code" data-failed={failed || undefined}>
      <input
        value={code}
        onChange={(event) =>
          onChange(event.target.value.replace(/\D/g, "").slice(0, 6))
        }
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label="Code"
        maxLength={6}
        disabled={disabled}
        autoFocus
      />
      {Array.from({ length: 6 }, (_, index) => (
        <span
          key={index}
          className="tile"
          data-current={
            (focused && !disabled && index === Math.min(code.length, 5)) ||
            undefined
          }
        >
          {code[index] ?? ""}
        </span>
      ))}
    </label>
  );
}
