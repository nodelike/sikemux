import { useSignIn, useSignUp } from "@clerk/react";
import {
  explain,
  signInFlow,
  type CodePurpose,
  type SignInStep,
} from "@shared/account/account";
import { IconArrowLeft, IconEye, IconEyeOff } from "@shared/ui/Icons";
import {
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type ReactNode,
  type SubmitEvent,
} from "react";

import { Gate, GateActions } from "./Frame.tsx";
import "./gate.css";
import { AppleMark, GitHubMark, GoogleMark } from "./icons.tsx";
import { useInviteOrg } from "./invite.ts";
import { brandMark, iconMark } from "./marks.ts";
import {
  DELETE_ACCOUNT,
  HOST,
  isPagePath,
  isSharePath,
  JOIN,
  noteSignUpReason,
  rememberReturn,
} from "./navigation.ts";
import { Btn, Problem, Txt } from "./ui.tsx";

type Provider = "oauth_apple" | "oauth_google" | "oauth_github";

type Action = "apple" | "google" | "github" | "submit" | "resend";

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

/** On a shared chat's or page's page or an invite a finished sign-in stays there, where what was shared or the invite then opens, instead of going to the account pages. */
function staying() {
  return isSharePath(location.pathname) ||
    isPagePath(location.pathname) ||
    location.pathname === HOST ||
    location.pathname === JOIN
    ? { navigate: () => undefined }
    : undefined;
}

export function SignIn({
  ready,
  intro,
  notice,
}: {
  ready: boolean;
  intro?: { title: string; lede: ReactNode };
  notice?: ReactNode;
}) {
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const [step, setStep] = useState<SignInStep>({ kind: "email" });
  useEffect(() => noteSignUpReason(), []);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState<Action>();
  const [problem, setProblem] = useState<string>();
  const [resent, setResent] = useState(false);
  const queued = useRef<{ action: Action; work: () => Promise<void> }>(null);
  const [inviteToken] = useState(() =>
    location.pathname === JOIN ? location.hash.slice(1) : "",
  );
  const joining = useInviteOrg(inviteToken);

  const go = (next: SignInStep) => {
    setProblem(undefined);
    setPassword("");
    setCode("");
    setResent(false);
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

  /** A sign-in left from an earlier visit, such as an abandoned provider page, would make the provider buttons do nothing. */
  const dropStale = useEffectEvent(() => {
    if (signIn.id) void signIn.reset();
  });
  useEffect(() => {
    if (ready) dropStale();
  }, [ready]);

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
    attempt("submit", () => flow.submitPassword(email, password));

  const submitNewAccount = () =>
    attempt("submit", () => flow.submitNewAccount(email, password));

  const submitNewPassword = () =>
    attempt("submit", () => flow.submitNewPassword(password));

  const forgot = () => attempt("submit", flow.forgot);

  const resend = (purpose: CodePurpose) =>
    attempt("resend", async () => {
      const answer =
        purpose === "trust"
          ? await signIn.mfa.sendEmailCode()
          : purpose === "reset"
            ? await signIn.resetPasswordEmailCode.sendCode()
            : await signUp.verifications.sendEmailCode();
      if (answer.error) throw answer.error;
      setResent(true);
    });

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

  const title =
    step.kind === "email"
      ? (intro?.title ??
        (joining ? `Sign in to join ${joining}` : "Sign in to Sikemux"))
      : step.kind === "code"
        ? "Check your email"
        : step.kind === "password"
          ? "Enter your password"
          : step.kind === "create"
            ? "Choose a password"
            : "Choose a new password";

  const lede =
    step.kind === "email" ? (
      joining && !intro ? (
        <>
          Already use Sikemux? Sign in the way you always do. The invite works
          with any account, so you don&rsquo;t need your {joining} address.
        </>
      ) : (
        (intro?.lede ??
        "One account for every device you use with Sikemux. Sign in on each, and they find each other.")
      )
    ) : step.kind === "code" ? (
      <>
        Sikemux sent a code to <b>{email}</b>, {CODE_DETAIL[step.purpose]}.
      </>
    ) : step.kind === "create" ? (
      <>
        A new account for <b>{email}</b>. At least 15 characters.
      </>
    ) : (
      email
    );

  const action =
    step.kind === "password"
      ? "Sign in"
      : step.kind === "create"
        ? "Create account"
        : "Save password";

  const mark =
    step.kind === "code"
      ? iconMark("mail")
      : step.kind !== "email"
        ? iconMark("key")
        : location.pathname === DELETE_ACCOUNT
          ? iconMark("warning")
          : location.pathname === HOST
            ? iconMark("laptop")
            : brandMark;

  return (
    <Gate
      title={title}
      lede={lede}
      mark={mark}
      scene={joining ? "blue" : location.pathname === HOST ? "teal" : "sign"}
      tone={
        busy !== undefined
          ? "waiting"
          : problem !== undefined
            ? "failed"
            : undefined
      }
      back={
        step.kind === "email" ? undefined : (
          <Txt className="gback" onClick={back}>
            <IconArrowLeft size={14} />
            Back
          </Txt>
        )
      }
      say={
        step.kind === "create"
          ? "A password found in a known breach is refused."
          : undefined
      }
    >
      {step.kind === "email" ? (
        <>
          {notice}
          <div className="ways">
            <Btn
              quiet={busy !== undefined && busy !== "apple"}
              onClick={() => withProvider("oauth_apple")}
              disabled={busy !== undefined}
              busy={busy === "apple"}
            >
              {busy === "apple" ? null : <AppleMark />}
              Continue with Apple
            </Btn>
            <Btn
              quiet={busy !== "google"}
              onClick={() => withProvider("oauth_google")}
              disabled={busy !== undefined}
              busy={busy === "google"}
            >
              {busy === "google" ? null : <GoogleMark />}
              Continue with Google
            </Btn>
            <Btn
              quiet={busy !== "github"}
              onClick={() => withProvider("oauth_github")}
              disabled={busy !== undefined}
              busy={busy === "github"}
            >
              {busy === "github" ? null : <GitHubMark />}
              Continue with GitHub
            </Btn>
          </div>
          <div className="orl" role="separator">
            <span>or</span>
          </div>
          <form onSubmit={submit}>
            <label className={problem ? "gf bad" : "gf"}>
              <span className="gfi">
                <input
                  type="email"
                  value={email}
                  onChange={(event) => {
                    setEmail(event.target.value);
                    if (problem) setProblem(undefined);
                  }}
                  placeholder="you@example.com"
                  autoComplete="email"
                  aria-label="Email"
                  required
                />
              </span>
            </label>
            <Problem>{problem}</Problem>
            <Btn
              quiet={busy !== "submit"}
              className="wide"
              type="submit"
              disabled={busy !== undefined}
              busy={busy === "submit"}
            >
              Continue with email
            </Btn>
          </form>
        </>
      ) : step.kind === "code" ? (
        <>
          <CodeEntry
            code={code}
            onChange={typeCode}
            disabled={busy !== undefined}
            failed={problem !== undefined && code.length === 0}
          />
          <Problem>{problem}</Problem>
          <GateActions>
            <Txt
              disabled={busy !== undefined}
              onClick={() => resend(step.purpose)}
            >
              {busy === "resend"
                ? "Sending…"
                : resent
                  ? "Code sent"
                  : "Resend code"}
            </Txt>
          </GateActions>
        </>
      ) : (
        <form onSubmit={submit}>
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
            onChange={(next) => {
              setPassword(next);
              if (problem) setProblem(undefined);
            }}
            autoComplete={
              step.kind === "password" ? "current-password" : "new-password"
            }
            bad={problem !== undefined}
          />
          {step.kind === "create" ? <div id="clerk-captcha" /> : null}
          <Problem>{problem}</Problem>
          <GateActions>
            <Btn
              type="submit"
              disabled={busy !== undefined}
              busy={busy === "submit"}
            >
              {busy === "submit" && step.kind === "create"
                ? "Creating"
                : action}
            </Btn>
            {step.kind === "password" ? (
              <Txt onClick={forgot} disabled={busy !== undefined}>
                Forgot your password?
              </Txt>
            ) : null}
          </GateActions>
        </form>
      )}
    </Gate>
  );
}

export function PasswordField({
  value,
  onChange,
  autoComplete,
  label = "Password",
  bad,
}: {
  value: string;
  onChange: (value: string) => void;
  autoComplete: string;
  label?: string;
  bad?: boolean;
}) {
  const [shown, setShown] = useState(false);
  return (
    <label className={bad ? "gf bad" : "gf"}>
      {label ? <span>{label}</span> : null}
      <span className="gfi">
        <input
          type={shown ? "text" : "password"}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          autoComplete={autoComplete}
          aria-label={label || "Password"}
          autoFocus
          required
        />
        <button
          type="button"
          onClick={() => setShown((was) => !was)}
          aria-label={shown ? "Hide password" : "Show password"}
        >
          {shown ? <IconEyeOff size={15} /> : <IconEye size={15} />}
        </button>
      </span>
    </label>
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
    <label className={`tiles${failed ? " bad" : ""}${disabled ? " off" : ""}`}>
      {Array.from({ length: 6 }, (_, index) => (
        <span
          key={index}
          className={
            focused && !disabled && index === Math.min(code.length, 5)
              ? "cur"
              : undefined
          }
        >
          {code[index] ?? ""}
        </span>
      ))}
      <input
        value={code}
        onChange={(event) =>
          onChange(event.target.value.replace(/\D/g, "").slice(0, 6))
        }
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label="Six digit code"
        maxLength={6}
        disabled={disabled}
        autoFocus
      />
    </label>
  );
}
