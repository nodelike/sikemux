import { useSignIn, useSignUp } from "@clerk/react";
import {
  explain,
  signInFlow,
  type CodePurpose,
  type SignInStep,
} from "@shared/account/account";
import { IconBack, IconEye, IconEyeOff } from "@shared/ui/Icons";
import {
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type ReactNode,
  type SubmitEvent,
} from "react";

import { Foot } from "./Frame.tsx";
import { AppleMark, GitHubMark, GoogleMark } from "./icons.tsx";
import { useInviteOrg } from "./invite.ts";
import { brandMark, iconMark } from "./marks.ts";
import {
  HOST,
  isPagePath,
  isSharePath,
  JOIN,
  noteSignUpReason,
  rememberReturn,
} from "./navigation.ts";
import { Btn, Field, Hero, Problem, Txt } from "./ui.tsx";

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
  const queued = useRef<{ action: Action; work: () => Promise<void> }>(null);
  const [inviteToken] = useState(() =>
    location.pathname === JOIN ? location.hash.slice(1) : "",
  );
  const joining = useInviteOrg(inviteToken);

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
        Sikemux sent a code to <span className="lit">{email}</span>,{" "}
        {CODE_DETAIL[step.purpose]}.
      </>
    ) : step.kind === "create" ? (
      <>
        A new account for <span className="lit">{email}</span>. At least 15
        characters; a password found in a known breach is refused.
      </>
    ) : (
      <span className="lit">{email}</span>
    );

  const action =
    step.kind === "password"
      ? "Sign in"
      : step.kind === "create"
        ? "Create account"
        : "Save password";

  return (
    <Hero
      title={title}
      lede={lede}
      mark={step.kind === "code" ? iconMark("mail") : brandMark}
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
          <Txt onClick={back}>
            <IconBack className="ic" />
            Back
          </Txt>
        )
      }
      foot={<Foot />}
    >
      {step.kind === "email" ? (
        <>
          {notice}
          <div className="do">
            <Btn
              onClick={() => withProvider("oauth_apple")}
              disabled={busy !== undefined}
              busy={busy === "apple"}
            >
              <AppleMark />
              Continue with Apple
            </Btn>
            <Btn
              quiet
              onClick={() => withProvider("oauth_google")}
              disabled={busy !== undefined}
              busy={busy === "google"}
            >
              <GoogleMark />
              Continue with Google
            </Btn>
            <Btn
              quiet
              onClick={() => withProvider("oauth_github")}
              disabled={busy !== undefined}
              busy={busy === "github"}
            >
              <GitHubMark />
              Continue with GitHub
            </Btn>
            <div className="or" role="separator">
              or
            </div>
          </div>
          <form className="do" onSubmit={submit}>
            <Field
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              placeholder="you@example.com"
              autoComplete="email"
              aria-label="Email"
              required
            />
            <Problem>{problem}</Problem>
            <Btn
              quiet
              type="submit"
              disabled={busy !== undefined}
              busy={busy === "submit"}
            >
              Continue with email
            </Btn>
          </form>
        </>
      ) : step.kind === "code" ? (
        <div className="do">
          <CodeEntry
            code={code}
            onChange={typeCode}
            disabled={busy !== undefined}
            failed={problem !== undefined && code.length === 0}
          />
          <Problem>{problem}</Problem>
        </div>
      ) : (
        <form className="do" onSubmit={submit}>
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
          <Problem>{problem}</Problem>
          <Btn
            type="submit"
            disabled={busy !== undefined}
            busy={busy === "submit"}
          >
            {action}
          </Btn>
          {step.kind === "password" ? (
            <div>
              <Txt onClick={forgot} disabled={busy !== undefined}>
                Forgot your password?
              </Txt>
            </div>
          ) : null}
        </form>
      )}
    </Hero>
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
    <div className="secret">
      <Field
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
        onClick={() => setShown((was) => !was)}
        aria-label={shown ? "Hide password" : "Show password"}
      >
        {shown ? <IconEyeOff size={16} /> : <IconEye size={16} />}
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
    <label className="digits" data-failed={failed || undefined}>
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
          className="digit"
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
