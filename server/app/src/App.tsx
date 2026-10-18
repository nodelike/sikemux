import { HandleSSOCallback, useAuth, useClerk, useUser } from "@clerk/react";
import { useEffect, useRef, useState, type MouseEvent } from "react";

import { Avatar } from "./Avatar.tsx";
import { Backdrop } from "./Backdrop.tsx";
import { Consent, CONSENT } from "./Consent.tsx";
import {
  AccountDeleted,
  DeleteAccount,
  DeleteAccountIntro,
} from "./DeleteAccount.tsx";
import { Devices, useDevices } from "./Devices.tsx";
import { HOST, HostApproval } from "./HostApproval.tsx";
import { Logo } from "./icons.tsx";
import { useLive } from "./live.ts";
import { RestoreAccount, useAccountState } from "./RestoreAccount.tsx";
import {
  appReturn,
  DELETE_ACCOUNT,
  isSharePath,
  takeReturn,
  takeShareReturn,
  usePath,
} from "./navigation.ts";
import { SignIn } from "./SignIn.tsx";
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
  const showDeleted =
    deleted !== null ||
    (path === DELETE_ACCOUNT &&
      new URLSearchParams(location.search).has("deleted") &&
      !signedIn);

  useEffect(() => {
    document.title = showDeleted
      ? "Account deleted · Sikemux"
      : path === "/sso-callback"
        ? "Signing in · Sikemux"
        : path === DELETE_ACCOUNT
          ? "Delete your account · Sikemux"
          : path === CONSENT
            ? "Sign in on your computer · Sikemux"
            : path === HOST
              ? "Add a host · Sikemux"
              : signedIn
                ? "Your devices · Sikemux"
                : "Sign in · Sikemux";
  }, [path, signedIn, showDeleted]);

  useEffect(() => {
    if (!signedIn || path === "/sso-callback") return;
    const share = takeShareReturn();
    if (share) location.replace(share);
  }, [signedIn, path]);

  const afterSignIn = () => {
    const to = takeReturn();
    if (isSharePath(to)) location.replace(to);
    else go(to, { replace: true });
  };
  const returning = isLoaded && isSignedIn ? appReturn() : null;

  useEffect(() => {
    if (returning) location.replace(returning);
  }, [returning]);

  /** Reached from this page or from the account's live connection, so it may run twice. */
  const onDeleted = (deletesAt?: string) => {
    if (signingOut.current) return;
    signingOut.current = true;
    setDeleted({ deletesAt });
    go(deletedPage(deletesAt), { replace: true });
    signOut({ redirectUrl: deletedPage(deletesAt) }).catch(() => undefined);
  };

  return (
    <>
      <Backdrop />
      <div className="page">
        {returning ? (
          <section className="panel">
            <p className="quiet">Taking you back to Sikemux…</p>
          </section>
        ) : showDeleted ? (
          <main className="center">
            <AccountDeleted
              deletesAt={deleted?.deletesAt ?? deletesAtOf(location.search)}
            />
          </main>
        ) : path === "/sso-callback" ? (
          <section className="panel">
            <p className="quiet">Signing you in…</p>
            <HandleSSOCallback
              navigateToApp={afterSignIn}
              navigateToSignIn={afterSignIn}
              navigateToSignUp={afterSignIn}
            />
          </section>
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
            deleting={path === DELETE_ACCOUNT}
            onDelete={follow(DELETE_ACCOUNT)}
            onBack={() => go("/")}
            onDeleted={onDeleted}
          />
        ) : (
          <main className="center">
            {path === DELETE_ACCOUNT ? <DeleteAccountIntro /> : null}
            <SignIn ready={isLoaded} />
            <footer className="legal">
              <a href="https://sikemux.com/privacy">Privacy</a>
              <a href="https://sikemux.com/terms">Terms</a>
            </footer>
          </main>
        )}
      </div>
    </>
  );
}

function Account({
  ready,
  deleting,
  onDelete,
  onBack,
  onDeleted,
}: {
  ready: boolean;
  deleting: boolean;
  onDelete: (event: MouseEvent<HTMLAnchorElement>) => void;
  onBack: () => void;
  onDeleted: (deletesAt?: string) => void;
}) {
  const { user } = useUser();
  const { isSignedIn } = useAuth();
  const { signOut } = useClerk();
  const email = user?.primaryEmailAddress?.emailAddress;
  const { account, restored } = useAccountState(ready);
  const active = account.state === "active";
  const devices = useDevices(ready && active);

  useLive(ready && isSignedIn === true && active, {
    onEvents: devices.apply,
    onResync: devices.reload,
    onAccountDeleted: () => onDeleted(),
  });

  return (
    <div className="account">
      <header className="top">
        <div className="brand">
          <Logo size={20} />
          <span>Sikemux</span>
        </div>
        <div className="who">
          <Avatar />
          {email ? <span className="email">{email}</span> : null}
          <button
            type="button"
            className="button small"
            onClick={() => void signOut({ redirectUrl: "/" })}
            disabled={!ready}
          >
            Sign out
          </button>
        </div>
      </header>
      <main>
        {account.state === "deleting" ? (
          <RestoreAccount deletesAt={account.deletesAt} onRestored={restored} />
        ) : deleting ? (
          <DeleteAccount
            email={email}
            restorableDays={
              account.state === "active" ? account.restorableDays : undefined
            }
            load={devices.load}
            onBack={onBack}
            onDeleted={onDeleted}
          />
        ) : (
          <>
            <h1>Your devices</h1>
            <p className="lede">
              Hosts and clients signed in to this account. A client still
              connects to a host only after someone at the host allows it.
            </p>
            <Devices load={devices.load} remove={devices.remove} />
            <footer className="account-footer">
              <a href={DELETE_ACCOUNT} onClick={onDelete}>
                Delete account
              </a>
            </footer>
          </>
        )}
      </main>
    </div>
  );
}
