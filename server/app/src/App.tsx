import {
  HandleSSOCallback,
  useAuth,
  useClerk,
  useSignIn,
  useUser,
} from "@clerk/react";
import {
  useEffect,
  useEffectEvent,
  useRef,
  useState,
  type MouseEvent,
} from "react";

import {
  IconActivity,
  IconCreditCard,
  IconGlobe,
  IconStack,
  IconPlug,
  IconServer,
  IconShield,
  IconTag,
  IconUser,
  IconUsers,
} from "@shared/ui/Icons";

import { AccountPage } from "./AccountPage.tsx";
import { Backdrop } from "./Backdrop.tsx";
import { ConnectorsPage } from "./Connectors.tsx";
import { BillingPage, useBillingShown } from "./Billing.tsx";
import { Consent, Panel, SIGNED_IN, SignedIn } from "./Consent.tsx";
import {
  AccountDeleted,
  DeleteAccount,
  DeleteAccountIntro,
} from "./DeleteAccount.tsx";
import { Devices, useDevices } from "./Devices.tsx";
import { FoldWaiting, useFoldGate } from "./Fold.tsx";
import { HostApproval } from "./HostApproval.tsx";
import { useLive } from "./live.ts";
import { PricingPage } from "./Pricing.tsx";
import { ShedNotHost } from "./Shed.tsx";
import { Pay } from "./Pay.tsx";
import {
  AuditPage,
  PolicyPage,
  DomainsPage,
  OfferBanner,
  TeamsPage,
  useOffers,
} from "./OrgPages.tsx";
import { JoinPage, TeamPage } from "./Team.tsx";
import { RestoreAccount, useAccountState } from "./RestoreAccount.tsx";
import {
  appReturn,
  AUDIT,
  POLICY,
  BILLING,
  CONNECTORS,
  CONSENT,
  DELETE_ACCOUNT,
  DOMAINS,
  HOST,
  HOSTS,
  isSharePath,
  JOIN,
  PAY,
  PRICING,
  takeReturn,
  TEAM,
  TEAMS,
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
  const appReturning = signedIn && fold.open ? appReturn() : null;

  useEffect(() => {
    if (appReturning && isLoaded) location.replace(appReturning);
  }, [appReturning, isLoaded]);

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
            <SsoCallback done={afterSignIn} />
          </main>
        ) : signedIn && path !== SIGNED_IN && !fold.open ? (
          <main className="center">{fold.screen}</main>
        ) : path === PAY ? (
          <main className="center">
            <Pay />
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
            <FoldWaiting />
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
  [CONNECTORS]: "Connectors",
  [HOSTS]: "Hosts",
  [BILLING]: "Plan and billing",
  [TEAM]: "Team",
  [TEAMS]: "Teams",
  [DOMAINS]: "Domains",
  [AUDIT]: "Audit log",
  [POLICY]: "Policy",
  [JOIN]: "Join an org",
  [PRICING]: "Pricing",
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
  const { account, staff, restored } = useAccountState(ready);
  const active = account.state === "active";
  const devices = useDevices(ready && active);
  const signIns = useSignIns(ready && active);
  const hostCount =
    devices.load.state === "loaded"
      ? devices.load.devices.filter((device) => device.role === "host").length
      : undefined;
  const billingShown = useBillingShown(ready && active);
  const [planVersion, setPlanVersion] = useState(0);
  const offers = useOffers(billingShown);
  const shown: Record<string, boolean> = {
    [BILLING]: billingShown,
    [TEAM]: billingShown,
    [TEAMS]: billingShown,
    [DOMAINS]: billingShown,
    [AUDIT]: billingShown,
    [POLICY]: billingShown,
    [JOIN]: billingShown,
    [PRICING]: staff,
  };
  const page = path in PAGE_TITLES && shown[path] !== false ? path : "/";

  useEffect(() => {
    document.title = `${PAGE_TITLES[page] ?? "Account"} · Sikemux`;
  }, [page]);

  useLive(ready && isSignedIn === true && active, {
    onEvents: (events) => {
      devices.apply(events);
      signIns.apply(events);
      if (events.some((event) => event.type === "plan.changed"))
        setPlanVersion((version) => version + 1);
    },
    onResync: () => {
      devices.reload();
      signIns.reload();
      setPlanVersion((version) => version + 1);
    },
    onAccountDeleted: () => onDeleted(),
  });

  const groups: NavGroup[] = [
    {
      items: [
        { path: "/", name: "Account", icon: <IconUser size={15} /> },
        {
          path: CONNECTORS,
          name: "Connectors",
          icon: <IconPlug size={15} />,
        },
        {
          path: HOSTS,
          name: "Hosts",
          icon: <IconServer size={15} />,
          count: hostCount ? String(hostCount) : undefined,
        },
      ],
    },
    ...(billingShown
      ? [
          {
            label: "Billing",
            items: [
              {
                path: BILLING,
                name: "Plan and billing",
                icon: <IconCreditCard size={15} />,
              },
            ],
          },
          {
            label: "Org",
            items: [
              { path: TEAM, name: "Team", icon: <IconUsers size={15} /> },
              { path: TEAMS, name: "Teams", icon: <IconStack size={15} /> },
              {
                path: DOMAINS,
                name: "Domains",
                icon: <IconGlobe size={15} />,
              },
              {
                path: AUDIT,
                name: "Audit log",
                icon: <IconActivity size={15} />,
              },
              {
                path: POLICY,
                name: "Policy",
                icon: <IconShield size={15} />,
              },
            ],
          },
        ]
      : []),
    ...(staff
      ? [
          {
            label: "Staff",
            items: [
              {
                path: PRICING,
                name: "Pricing",
                icon: <IconTag size={15} />,
              },
            ],
          },
        ]
      : []),
  ];

  return (
    <Shell
      path={page === DELETE_ACCOUNT ? "/" : page === JOIN ? TEAM : page}
      follow={follow}
      groups={groups}
      wide={page === PRICING}
    >
      {page !== TEAM && page !== JOIN && account.state === "active" ? (
        <OfferBanner offers={offers.offers} onOpen={() => go(TEAM)} />
      ) : null}
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
      ) : page === JOIN ? (
        <JoinPage follow={follow} />
      ) : page === TEAM ? (
        <TeamPage follow={follow} />
      ) : page === TEAMS ? (
        <TeamsPage />
      ) : page === DOMAINS ? (
        <DomainsPage />
      ) : page === AUDIT ? (
        <AuditPage />
      ) : page === POLICY ? (
        <PolicyPage />
      ) : page === PRICING ? (
        <PricingPage />
      ) : page === BILLING ? (
        <BillingPage version={planVersion} staff={staff} />
      ) : page === HOSTS ? (
        <>
          <PageHead title="Hosts">
            Every host you own, linked on one account. No plan caps how many. A
            client connects to a host only after someone at the host allows it.
          </PageHead>
          <Devices load={devices.load} remove={devices.remove} />
          {staff ? <ShedNotHost follow={follow} /> : null}
        </>
      ) : page === CONNECTORS ? (
        <ConnectorsPage signIns={signIns.load} />
      ) : (
        <AccountPage onDelete={follow(DELETE_ACCOUNT)} />
      )}
    </Shell>
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
