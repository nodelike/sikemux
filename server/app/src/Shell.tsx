import lockup from "../../../brand/wordmark/lockup-purple.svg";
import { useClerk, useUser } from "@clerk/react";
import type { Org } from "@sikemux/protocol";
import {
  IconCreditCard,
  IconGlobe,
  IconLogOut,
  IconPlus,
  IconScrollText,
  IconShieldCheck,
  IconUsers,
} from "@shared/ui/Icons";
import {
  useEffect,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";

import { Avatar, OrgAvatar } from "./Avatar.tsx";
import { ORG_PAGES, orgPath, START, type OrgPage } from "./navigation.ts";
import { frozen, seesPage } from "./orgs.ts";

export type NavItem = {
  path: string;
  name: string;
  icon: ReactNode;
  count?: number;
};

const ORG_ICONS: Record<OrgPage, ReactNode> = {
  people: <IconUsers size={15} />,
  domains: <IconGlobe size={15} />,
  "audit-log": <IconScrollText size={15} />,
  policy: <IconShieldCheck size={15} />,
  billing: <IconCreditCard size={15} />,
};

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;

/** The signed-in pages: one rail of You, each org and its pages, or two strips across the top on a phone. */
export function Shell({
  path,
  follow,
  you,
  staff,
  orgs,
  current,
  canStart,
  offered,
  children,
}: {
  path: string;
  follow: Follow;
  you: NavItem[];
  staff: NavItem[];
  orgs: Org[];
  current?: { org: Org; page: OrgPage };
  canStart: boolean;
  offered: boolean;
  children: ReactNode;
}) {
  const orgsShown = orgs.length > 0 || canStart;
  const tabs = current
    ? pagesOf(current.org).map(([page, name]) => ({
        path: orgPath(current.org.id, page),
        name,
      }))
    : [...you, ...staff];
  return (
    <div className="shell">
      <aside className="rail">
        <a className="lk" href="/" onClick={follow("/")}>
          <img src={lockup} alt="Sikemux" />
        </a>
        <nav aria-label="Pages">
          <Group label="You">
            {you.map((item) => (
              <NavLink key={item.path} {...item} at={path} follow={follow} />
            ))}
          </Group>
          {orgsShown ? (
            <div className={orgs.length > 4 ? "grp many" : "grp"}>
              <div className="lab">Orgs</div>
              <div className="orgs">
                {orgs.map((org) => (
                  <OrgLinks
                    key={org.id}
                    org={org}
                    page={current?.org.id === org.id ? current.page : undefined}
                    follow={follow}
                  />
                ))}
              </div>
              {canStart ? (
                <a
                  className={path === START ? "ol on" : "ol"}
                  href={START}
                  onClick={follow(START)}
                >
                  <span className="plus">
                    <IconPlus size={15} />
                  </span>
                  <span className="n">Start or join an org</span>
                  {offered ? (
                    <i
                      className="dot new"
                      title="An org matches your email domain"
                    />
                  ) : null}
                </a>
              ) : null}
            </div>
          ) : null}
          {staff.length ? (
            <Group label="Staff">
              {staff.map((item) => (
                <NavLink key={item.path} {...item} at={path} follow={follow} />
              ))}
            </Group>
          ) : null}
        </nav>
        <Me />
      </aside>
      <main className="main">
        <div className="phonebar">
          <Scopes
            orgs={orgs}
            current={current?.org}
            canStart={canStart}
            follow={follow}
          />
          <Tabs tabs={tabs} at={path} follow={follow} />
        </div>
        {children}
      </main>
    </div>
  );
}

function pagesOf(org: Org) {
  return (Object.entries(ORG_PAGES) as [OrgPage, string][]).filter(([page]) =>
    seesPage(org, page),
  );
}

function Group({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grp">
      <div className="lab">{label}</div>
      {children}
    </div>
  );
}

function NavLink({
  path,
  name,
  icon,
  count,
  at,
  follow,
}: NavItem & { at: string; follow: Follow }) {
  return (
    <a
      className="rl"
      href={path}
      onClick={follow(path)}
      aria-current={path === at ? "page" : undefined}
    >
      {icon}
      <span className="n">{name}</span>
      {count ? <span className="count">{count}</span> : null}
    </a>
  );
}

function OrgLinks({
  org,
  page,
  follow,
}: {
  org: Org;
  page?: OrgPage;
  follow: Follow;
}) {
  const row = useRef<HTMLDivElement>(null);
  const here = page !== undefined;
  useEffect(() => {
    if (here) row.current?.scrollIntoView({ block: "nearest" });
  }, [here]);
  const to = orgPath(org.id, page ?? "people");
  return (
    <div className="og" ref={row}>
      <a className={here ? "ol on" : "ol"} href={to} onClick={follow(to)}>
        <OrgAvatar id={org.id} />
        <span className="n">{org.name}</span>
        {frozen(org) ? <FrozenDot /> : null}
      </a>
      {here ? (
        <div className="sub">
          {pagesOf(org).map(([each, name]) => (
            <NavLink
              key={each}
              path={orgPath(org.id, each)}
              name={name}
              icon={ORG_ICONS[each]}
              at={orgPath(org.id, page)}
              follow={follow}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function FrozenDot() {
  return <i className="dot" title="Its Team plan ended" />;
}

function Scopes({
  orgs,
  current,
  canStart,
  follow,
}: {
  orgs: Org[];
  current?: Org;
  canStart: boolean;
  follow: Follow;
}) {
  const strip = useRef<HTMLDivElement>(null);
  useEffect(() => {
    strip.current
      ?.querySelector("[aria-current=true]")
      ?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [current?.id]);
  if (!orgs.length && !canStart) return null;
  return (
    <div className={orgs.length ? "scopes" : "scopes none"} ref={strip}>
      <a
        className="sc"
        href="/"
        onClick={follow("/")}
        aria-current={!current}
        aria-label="You"
      >
        <Avatar size={20} />
        <span>You</span>
      </a>
      <i className="bar1" />
      {orgs.map((org) => (
        <a
          key={org.id}
          className="sc"
          href={orgPath(org.id)}
          onClick={follow(orgPath(org.id))}
          aria-current={org.id === current?.id}
          aria-label={org.name}
        >
          <OrgAvatar id={org.id} />
          <span>{org.name}</span>
          {frozen(org) ? <FrozenDot /> : null}
        </a>
      ))}
      {canStart ? (
        <a
          className="sc add"
          href={START}
          onClick={follow(START)}
          aria-label="Start or join an org"
        >
          <IconPlus size={15} />
          <span>Start or join an org</span>
        </a>
      ) : null}
    </div>
  );
}

function Tabs({
  tabs,
  at,
  follow,
}: {
  tabs: { path: string; name: string }[];
  at: string;
  follow: Follow;
}) {
  const strip = useRef<HTMLElement>(null);
  useEffect(() => {
    strip.current
      ?.querySelector("[aria-current]")
      ?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [at]);
  return (
    <nav className="tabs" aria-label="Pages" ref={strip}>
      {tabs.map((tab) => (
        <a
          key={tab.path}
          className="tb"
          href={tab.path}
          onClick={follow(tab.path)}
          aria-current={tab.path === at ? "page" : undefined}
        >
          {tab.name}
        </a>
      ))}
    </nav>
  );
}

function Me() {
  const { user } = useUser();
  const { signOut } = useClerk();
  const [leaving, setLeaving] = useState(false);
  const leave = () => {
    setLeaving(true);
    signOut({ redirectUrl: "/" }).catch(() => setLeaving(false));
  };
  return (
    <div className="me">
      <Avatar size={22} />
      <span className="n">
        {user?.fullName || user?.username || "Your account"}
      </span>
      <button
        type="button"
        className="out"
        onClick={leave}
        disabled={leaving}
        data-busy={leaving || undefined}
        aria-label="Sign out"
        title="Sign out"
      >
        <IconLogOut size={15} />
      </button>
    </div>
  );
}
