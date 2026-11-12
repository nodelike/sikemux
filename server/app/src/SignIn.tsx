import { useSignIn, useSignUp } from "@clerk/react";
import {
  explain,
  signInFlow,
  type CodePurpose,
  type SignInStep,
} from "@shared/account/account";
import { IconBack, IconEye, IconEyeOff } from "@shared/ui/Icons";
import { useEffect, useRef, useState, type SubmitEvent } from "react";

import { AppleMark, GitHubMark, GoogleMark, Logo } from "./icons.tsx";
import { HOST, isSharePath, rememberReturn } from "./navigation.ts";

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
  const [step, setStep] = useState<SignInStep>({ kind: "email" });
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState<Action>();
  const [problem, setProblem] = useState<string>();
  const queued = useRef<{ action: Action; work: () => Promise<void> }>(null);

  const go = (next: SignInStep) => {
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

  const flow = signInFlow({
    signIn,
    signUp,
    go,
    finalize: staying,
    unsupported: {
      signIn: "This account needs a sign-in step this page does not do yet.",
      signUp: "The account needs more than this page asks for; try again.",
    },
  });

  const withProvider = (strategy: Provider) =>
    attempt(ACTIONS[strategy], async () => {
      rememberReturn();
      const { error } = await signIn.sso({
        strategy,
        redirectUrl: new URL("/", location.href).href,
        redirectCallbackUrl: new URL("/sso-callback", location.href).href,
      });
      if (error) throw error;
    });

  const submitEmail = () => attempt("submit", () => flow.submitEmail(email));

  const submitPassword = () =>
    attempt("submit", () => flow.submitPassword(password));

  const submitNewAccount = () =>
    attempt("submit", () => flow.submitNewAccount(email, password));

  const submitNewPassword = () =>
    attempt("submit", () => flow.submitNewPassword(password));

  const forgot = () => attempt("submit", flow.forgot);

  const submitCode = (purpose: CodePurpose, typed: string) =>
    attempt("submit", async () => {
      try {
        await flow.submitCode(purpose, typed);
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
    go({ kind: "email" });
  };

  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy) return;
    if (step.kind === "email") return submitEmail();
    if (step.kind === "password") return submitPassword();
    if (step.kind === "create") return submitNewAccount();
    if (step.kind === "new-password") return submitNewPassword();
  };

  if (step.kind === "email")
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
      <IconBack />
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
        {shown ? <IconEyeOff size={18} /> : <IconEye size={18} />}
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
