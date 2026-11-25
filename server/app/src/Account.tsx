import { useAuth, useUser } from "@clerk/react";
import {
  IconCreditCard,
  IconLaptop,
  IconPlug,
  IconTag,
  IconUser,
} from "@shared/ui/Icons";
import { useEffect, useEffectEvent, useState, type MouseEvent } from "react";

import { AccountPage } from "./AccountPage.tsx";
import { AuditLogPage } from "./AuditLog.tsx";
import { BillingPage, OrgBillingPage, useBillingShown } from "./Billing.tsx";
import { ConnectorsPage } from "./Connectors.tsx";
import { DeleteAccount } from "./DeleteAccount.tsx";
import { DetailsPage } from "./Details.tsx";
import { HostsPage, useDevices } from "./Devices.tsx";
import { DomainsPage } from "./Domains.tsx";
import { JoinPage } from "./Join.tsx";
import { useLive } from "./live.ts";
import { iconMark } from "./marks.ts";
import {
  BILLING,
  CONNECTORS,
  DELETE_ACCOUNT,
  HOSTS,
  JOIN,
  ORG_PAGES,
  orgPath,
  orgRoute,
  PRICING,
  START,
  TEAM,
} from "./navigation.ts";
import { keepLastOrg, lastOrg, seesPage, useOffers, useOrgs } from "./orgs.ts";
import { PeoplePage } from "./People.tsx";
import { PolicyPage } from "./Policy.tsx";
import { PricingPage } from "./Pricing.tsx";
import { RestoreAccount, useAccountState } from "./RestoreAccount.tsx";
import { Shell, type NavItem } from "./Shell.tsx";
import { useSignIns } from "./SignIns.tsx";
import { StartPage } from "./Start.tsx";
import { Section, Page, Problem } from "./ui.tsx";

const TITLES: Record<string, string> = {
  "/": "Account",
  [CONNECTORS]: "Connectors",
  [HOSTS]: "Hosts",
  [BILLING]: "Billing",
  [PRICING]: "Pricing",
  [START]: "Start or join an org",
  [JOIN]: "Join an org",
  [DELETE_ACCOUNT]: "Delete your account",
};

/** Everything a signed-in person reaches: their own pages, their orgs' and the ones that fill the window. */
export function Account({
  ready,
  path,
  follow,
  go,
  onDeleted,
}: {
  ready: boolean;
  path: string;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  go: (to: string, options?: { replace?: boolean }) => void;
  onDeleted: (deletesAt?: string) => void;
}) {
  const { user } = useUser();
  const { isSignedIn } = useAuth();
  const email = user?.primaryEmailAddress?.emailAddress;
  const [planVersion, setPlanVersion] = useState(0);
  const { account, staff, team, restored } = useAccountState(
    ready,
    planVersion,
  );
  const active = account.state === "active";
  const devices = useDevices(ready && active);
  const signIns = useSignIns(ready && active);
  const billingShown = useBillingShown(ready && active, planVersion);
  const teamShown = team !== "shut";
  const canStart = team === "sold" || team === "preview";
  const offers = useOffers(canStart);
  const route = orgRoute(path);
  const orgs = useOrgs(teamShown, `${route?.id ?? ""} ${planVersion}`);
  const list = teamShown ? (orgs.list ?? []) : [];
  const org = list.find((each) => each.id === route?.id);
  const current =
    route && org
      ? {
          org,
          page: seesPage(org, route.page, billingShown === true)
            ? route.page
            : ("people" as const),
        }
      : undefined;
  const named = path === TEAM || route !== null;
  const settled =
    account.state !== "loading" &&
    billingShown !== undefined &&
    (!teamShown || orgs.list !== undefined || orgs.problem !== undefined);

  /** A path that names no page here goes to the nearest one there is. */
  const settle = useEffectEvent(() => {
    const last = lastOrg(list);
    if (path === TEAM)
      go(last ? orgPath(last.id) : canStart ? START : "/", { replace: true });
    else if (route && !org && orgs.list) go("/", { replace: true });
    else if (current && route && current.page !== route.page)
      go(orgPath(current.org.id, current.page), { replace: true });
    else if (route && !teamShown) go("/", { replace: true });
  });
  useEffect(() => {
    if (settled && named && !(orgs.problem && !orgs.list)) settle();
  }, [settled, named, path, orgs.list, orgs.problem, current?.page]);

  const currentId = current?.org.id;
  useEffect(() => {
    if (currentId) keepLastOrg(currentId);
  }, [currentId]);

  const shown: Record<string, boolean> = {
    [BILLING]: billingShown === true,
    [PRICING]: staff,
    [START]: canStart,
  };
  const page =
    path in TITLES && shown[path] !== false
      ? path
      : path === BILLING && !settled
        ? path
        : "/";
  const title = current
    ? `${ORG_PAGES[current.page]} · ${current.org.name}`
    : (TITLES[page] ?? "Account");
  useEffect(() => {
    document.title = `${title} · Sikemux`;
  }, [title]);

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

  if (account.state === "deleting")
    return (
      <RestoreAccount deletesAt={account.deletesAt} onRestored={restored} />
    );
  if (page === DELETE_ACCOUNT)
    return (
      <DeleteAccount
        email={email}
        restorableDays={
          account.state === "active" ? account.restorableDays : undefined
        }
        load={devices.load}
        onBack={() => go("/")}
        onDeleted={onDeleted}
      />
    );
  const joined = async (id: string) => {
    await Promise.all([orgs.reload(), offers.reload()]);
    go(orgPath(id));
  };
  if (page === JOIN)
    return (
      <JoinPage
        canPaste={settled ? canStart : undefined}
        follow={follow}
        onHome={() => go("/", { replace: true })}
        onJoined={joined}
      />
    );
  if (page === START)
    return (
      <StartPage
        team={team}
        staff={staff}
        orgs={list}
        offers={offers.offers}
        follow={follow}
        onJoined={joined}
      />
    );

  const hosts =
    devices.load.state === "loaded"
      ? devices.load.devices.filter((device) => device.role === "host").length
      : 0;
  const you: NavItem[] = [
    { path: "/", name: "Account", icon: <IconUser size={15} /> },
    { path: CONNECTORS, name: "Connectors", icon: <IconPlug size={15} /> },
    {
      path: HOSTS,
      name: "Hosts",
      icon: <IconLaptop size={15} />,
      count: hosts,
    },
  ];
  if (billingShown)
    you.push({
      path: BILLING,
      name: "Billing",
      icon: <IconCreditCard size={15} />,
    });

  const body = current ? (
    current.page === "people" ? (
      <PeoplePage
        key={current.org.id}
        org={current.org}
        team={team}
        version={planVersion}
        follow={follow}
        reload={orgs.reload}
        onLeft={async () => {
          await orgs.reload();
          go("/", { replace: true });
        }}
      />
    ) : current.page === "domains" ? (
      <DomainsPage key={current.org.id} org={current.org} team={team} />
    ) : current.page === "audit-log" ? (
      <AuditLogPage key={current.org.id} org={current.org} />
    ) : current.page === "policy" ? (
      <PolicyPage key={current.org.id} org={current.org} team={team} />
    ) : current.page === "details" ? (
      <DetailsPage
        key={current.org.id}
        org={current.org}
        team={team}
        reload={orgs.reload}
      />
    ) : (
      <OrgBillingPage
        key={current.org.id}
        org={current.org}
        version={planVersion}
        team={team}
      />
    )
  ) : named ? (
    <Page title="Org" mark={iconMark("org")} scene="you">
      <Section busy={!orgs.problem}>
        <Problem>{orgs.problem}</Problem>
      </Section>
    </Page>
  ) : page === PRICING ? (
    <PricingPage orgs={list} reload={orgs.reload} />
  ) : page === BILLING ? (
    <BillingPage
      version={planVersion}
      staff={staff}
      team={team}
      orgs={list}
      follow={follow}
    />
  ) : page === HOSTS ? (
    <HostsPage
      load={devices.load}
      remove={devices.remove}
      staff={staff}
      follow={follow}
    />
  ) : page === CONNECTORS ? (
    <ConnectorsPage signIns={signIns.load} />
  ) : (
    <AccountPage team={team} onDelete={follow(DELETE_ACCOUNT)} />
  );

  return (
    <Shell
      path={current ? path : named ? "" : page}
      follow={follow}
      you={you}
      staff={
        staff
          ? [{ path: PRICING, name: "Pricing", icon: <IconTag size={15} /> }]
          : []
      }
      orgs={list}
      current={current}
      team={team}
      billed={billingShown === true}
      canStart={canStart}
      offered={offers.offers.length > 0}
    >
      {body}
    </Shell>
  );
}
