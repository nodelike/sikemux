import { HandleSSOCallback, useAuth, useClerk, useUser } from "@clerk/react";
import {
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type MouseEvent,
} from "react";

import { IconServer, IconUser } from "@shared/ui/Icons";

import { AccountPage } from "./AccountPage.tsx";
import { Backdrop } from "./Backdrop.tsx";
import { Consent, Panel, SIGNED_IN, SignedIn } from "./Consent.tsx";
import {
  AccountDeleted,
  DeleteAccount,
  DeleteAccountIntro,
} from "./DeleteAccount.tsx";
import { Devices, useDevices } from "./Devices.tsx";
import { HostApproval } from "./HostApproval.tsx";
import { useLive } from "./live.ts";
import { RestoreAccount, useAccountState } from "./RestoreAccount.tsx";
import {
  appReturn,
  CONSENT,
  DELETE_ACCOUNT,
  HOST,
  HOSTS,
  isSharePath,
  takeReturn,
  usePath,
} from "./navigation.ts";
import { SignIn } from "./SignIn.tsx";
import { Shell, type NavGroup } from "./Shell.tsx";
import { useSignIns } from "./SignIns.tsx";
import { PageHead } from "./ui.tsx";
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

  const shellShown =
    signedIn &&
    !showDeleted &&
    ![SIGNED_IN, CONSENT, HOST, "/sso-callback"].includes(path);

  useEffect(() => {
    if (shellShown) return;
    document.title = showDeleted
      ? "Account deleted · Sikemux"
      : path === "/sso-callback"
        ? "Signing in · Sikemux"
        : path === DELETE_ACCOUNT
          ? "Delete your account · Sikemux"
          : path === CONSENT || path === SIGNED_IN
            ? "Sign in on your computer · Sikemux"
            : path === HOST
              ? "Add a host · Sikemux"
              : "Sign in · Sikemux";
  }, [path, showDeleted, shellShown]);

  const returnTo = (to: string) => {
    if (isSharePath(to)) location.replace(to);
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
  const appReturning = signedIn ? appReturn() : null;

  useEffect(() => {
    if (appReturning && isLoaded) location.replace(appReturning);
  }, [appReturning, isLoaded]);

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
            <Panel>
              <p className="quiet">Signing you in…</p>
              <HandleSSOCallback
                navigateToApp={afterSignIn}
                navigateToSignIn={afterSignIn}
                navigateToSignUp={afterSignIn}
              />
            </Panel>
          </main>
        ) : path === SIGNED_IN ? (
          <main className="center">
            <SignedIn />
          </main>
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

const PAGE_TITLES: Record<string, string> = {
  [HOSTS]: "Hosts",
  [DELETE_ACCOUNT]: "Delete your account",
};

function Account({
  ready,
  path,
  follow,
  go,
  onDeleted,
}: {
  ready: boolean;
  path: string;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  go: (to: string) => void;
  onDeleted: (deletesAt?: string) => void;
}) {
  const { user } = useUser();
  const { isSignedIn } = useAuth();
  const email = user?.primaryEmailAddress?.emailAddress;
  const { account, restored } = useAccountState(ready);
  const active = account.state === "active";
  const devices = useDevices(ready && active);
  const signIns = useSignIns(ready && active);
  const hostCount =
    devices.load.state === "loaded"
      ? devices.load.devices.filter((device) => device.role === "host").length
      : undefined;
  const page = path in PAGE_TITLES ? path : "/";

  useEffect(() => {
    document.title = `${PAGE_TITLES[page] ?? "Account"} · Sikemux`;
  }, [page]);

  useLive(ready && isSignedIn === true && active, {
    onEvents: (events) => {
      devices.apply(events);
      signIns.apply(events);
    },
    onResync: () => {
      devices.reload();
      signIns.reload();
    },
    onAccountDeleted: () => onDeleted(),
  });

  const groups: NavGroup[] = [
    {
      items: [
        { path: "/", name: "Account", icon: <IconUser size={15} /> },
        {
          path: HOSTS,
          name: "Hosts",
          icon: <IconServer size={15} />,
          count: hostCount ? String(hostCount) : undefined,
        },
      ],
    },
  ];

  return (
    <Shell
      path={page === DELETE_ACCOUNT ? "/" : page}
      follow={follow}
      groups={groups}
    >
      {account.state === "deleting" ? (
        <RestoreAccount deletesAt={account.deletesAt} onRestored={restored} />
      ) : page === DELETE_ACCOUNT ? (
        <DeleteAccount
          email={email}
          restorableDays={
            account.state === "active" ? account.restorableDays : undefined
          }
          load={devices.load}
          onBack={() => go("/")}
          onDeleted={onDeleted}
        />
      ) : page === HOSTS ? (
        <>
          <PageHead title="Hosts">
            Every host you own, linked on one account. No plan caps how many. A
            client connects to a host only after someone at the host allows it.
          </PageHead>
          <Devices load={devices.load} remove={devices.remove} />
        </>
      ) : (
        <AccountPage signIns={signIns.load} onDelete={follow(DELETE_ACCOUNT)} />
      )}
    </Shell>
  );
}
