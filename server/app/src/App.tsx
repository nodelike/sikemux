import { HandleSSOCallback, useAuth, useClerk, useSignIn } from "@clerk/react";
import { useEffect, useEffectEvent, useRef, useState } from "react";

import { Account } from "./Account.tsx";
import { Consent, Panel } from "./Consent.tsx";
import { AccountDeleted, DeleteAccountIntro } from "./DeleteAccount.tsx";
import { FoldWaiting, useFoldGate } from "./Fold.tsx";
import { Field } from "./Field.tsx";
import { HostApproval } from "./HostApproval.tsx";
import { Pay } from "./Pay.tsx";
import { SIGNED_IN, SignedIn } from "./SignedIn.tsx";
import {
  appReturn,
  CONSENT,
  DELETE_ACCOUNT,
  HOST,
  isPagePath,
  isSharePath,
  PAY,
  takeReturn,
  usePath,
} from "./navigation.ts";
import { SignIn } from "./SignIn.tsx";
import { Toasts } from "./ui.tsx";
import "./app.css";

/** The deleted page's address, carrying when the account is deleted for good so it survives signing out. */
function deletedPage(deletesAt?: string): string {
  return `${DELETE_ACCOUNT}?deleted${deletesAt ? `=${encodeURIComponent(deletesAt)}` : ""}`;
}

function deletesAtOf(search: string): string | undefined {
  const value = new URLSearchParams(search).get("deleted");
  return value && Number.isFinite(Date.parse(value)) ? value : undefined;
}

/** Clerk leaves a nonzero `__client_uat` cookie once signed in, so the first paint can guess the right screen. */
const SIGNED_IN_BEFORE = /(?:^|;\s*)__client_uat(?:_\w+)?=[1-9]/.test(
  document.cookie,
);

export function App() {
  const { isLoaded, isSignedIn } = useAuth();
  const { signOut } = useClerk();
  const { path, go, follow } = usePath();
  const [deleted, setDeleted] = useState<{ deletesAt?: string } | null>(null);
  const signingOut = useRef(false);
  const signedIn = isLoaded ? isSignedIn : SIGNED_IN_BEFORE;
  /** Reached from this page or from the account's live connection, so it may run twice. */
  const onDeleted = (deletesAt?: string) => {
    if (signingOut.current) return;
    signingOut.current = true;
    setDeleted({ deletesAt });
    go(deletedPage(deletesAt), { replace: true });
    signOut({ redirectUrl: deletedPage(deletesAt) }).catch(() => undefined);
  };

  const fold = useFoldGate(signedIn && isLoaded, onDeleted);
  const showDeleted =
    deleted !== null ||
    (path === DELETE_ACCOUNT &&
      new URLSearchParams(location.search).has("deleted") &&
      !signedIn);

  const shellShown =
    signedIn &&
    fold.open &&
    !showDeleted &&
    ![SIGNED_IN, CONSENT, HOST, PAY, "/sso-callback"].includes(path);

  useEffect(() => {
    if (shellShown) return;
    document.title = showDeleted
      ? "Account deleted · Sikemux"
      : path === "/sso-callback"
        ? "Signing in · Sikemux"
        : path === DELETE_ACCOUNT
          ? "Delete your account · Sikemux"
          : path === PAY
            ? "Checkout · Sikemux"
            : path === CONSENT || path === SIGNED_IN
              ? "Sign in on your computer · Sikemux"
              : path === HOST
                ? "Add a host · Sikemux"
                : "Sign in · Sikemux";
  }, [path, showDeleted, shellShown]);

  const returnTo = (to: string) => {
    if (isSharePath(to) || isPagePath(to)) location.replace(to);
    else go(to, { replace: true });
  };
  const afterSignIn = () => returnTo(takeReturn() ?? "/");

  /** Clerk skips the callback page when the provider signs the person in at once, landing here instead. */
  const resumeReturn = useEffectEvent(() => {
    const to = takeReturn();
    if (to) returnTo(to);
  });

  useEffect(() => {
    if (signedIn && path !== "/sso-callback") resumeReturn();
  }, [signedIn, path]);
  const appReturning = signedIn && fold.open ? appReturn() : null;

  useEffect(() => {
    if (appReturning && isLoaded) location.replace(appReturning);
  }, [appReturning, isLoaded]);

  return (
    <Field>
      <Toasts>
        {appReturning ? (
          <main className="center">
            <Panel>
              <p className="quiet">Loading…</p>
            </Panel>
          </main>
        ) : showDeleted ? (
          <main className="center">
            <AccountDeleted
              deletesAt={deleted?.deletesAt ?? deletesAtOf(location.search)}
            />
          </main>
        ) : path === "/sso-callback" ? (
          <main className="center">
            <SsoCallback done={afterSignIn} />
          </main>
        ) : signedIn && path !== SIGNED_IN && !fold.open ? (
          <main className="center">{fold.screen}</main>
        ) : path === PAY ? (
          <main className="center">
            <Pay />
          </main>
        ) : path === SIGNED_IN ? (
          <SignedIn />
        ) : path === CONSENT ? (
          <main className="center">
            {signedIn ? <Consent /> : <SignIn ready={isLoaded} />}
          </main>
        ) : path === HOST ? (
          <main className="center">
            {signedIn ? <HostApproval /> : <SignIn ready={isLoaded} />}
          </main>
        ) : signedIn ? (
          <Account
            ready={isLoaded}
            path={path}
            follow={follow}
            go={go}
            onDeleted={onDeleted}
          />
        ) : (
          <main className="center">
            {path === DELETE_ACCOUNT ? <DeleteAccountIntro /> : null}
            <FoldWaiting />
            <SignIn ready={isLoaded} />
            <footer className="legal">
              <a href="https://sikemux.com/privacy">Privacy</a>
              <a href="https://sikemux.com/terms">Terms</a>
            </footer>
          </main>
        )}
      </Toasts>
    </Field>
  );
}

/** A new account may first ask the person to prove they're human, in the widget Clerk draws inside the callback. */
function SsoCallback({ done }: { done: () => void }) {
  const { signIn } = useSignIn();
  return (
    <Panel>
      <p className="quiet">
        {signIn.isTransferable ? "Creating your account…" : "Signing you in…"}
      </p>
      <HandleSSOCallback
        navigateToApp={done}
        navigateToSignIn={done}
        navigateToSignUp={done}
      />
      <a className="link" href="/">
        Back to sign in
      </a>
    </Panel>
  );
}
