import logo from "../../../brand/mark/mark-purple.svg";
import { useUser } from "@clerk/react";
import type { Org, OrgRole, TeamAccess } from "@sikemux/protocol";
import {
  IconCreditCard,
  IconGlobe,
  IconMore,
  IconPlus,
  IconScrollText,
  IconSettings,
  IconShieldCheck,
  IconTag,
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
import { frozen, renewable, seesPage } from "./orgs.ts";
import { Dialog } from "./ui.tsx";

export type NavItem = {
  path: string;
  name: string;
  icon: ReactNode;
  count?: number;
};

const ORG_ICONS: Record<OrgPage, ReactNode> = {
  people: <IconUsers size={15} />,
  groups: <IconTag size={15} />,
  domains: <IconGlobe size={15} />,
  policy: <IconShieldCheck size={15} />,
  billing: <IconCreditCard size={15} />,
  "audit-log": <IconScrollText size={15} />,
  details: <IconSettings size={15} />,
};

const YOU_ARE: Record<OrgRole, string> = {
  owner: "You're an owner",
  admin: "You're an admin",
  user: "You're a member",
};

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;

/** The signed-in pages: a strip of you and each org, the pages of the one shown beside it, and the page. On a phone the strip runs across the top and the pages along the bottom. */
export function Shell({
  path,
  follow,
  you,
  staff,
  orgs,
  current,
  team,
  billed,
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
  team: TeamAccess;
  billed: boolean;
  canStart: boolean;
  offered: boolean;
  children: ReactNode;
}) {
  const { user } = useUser();
  const starting = path === START;
  const items = starting
    ? []
    : current
      ? pagesOf(current.org, billed)
      : [...you, ...staff];
  return (
    <div className={starting ? "shell alone" : "shell"}>
      <Scopes
        at={current?.org.id ?? (starting ? START : "you")}
        orgs={orgs}
        team={team}
        canStart={canStart}
        offered={offered}
        follow={follow}
      />
      {starting ? null : (
        <aside className="rail">
          <div className="rh">
            <b>
              {current?.org.name ??
                (user?.fullName || user?.username || "Your account")}
            </b>
            <small>
              {current ? YOU_ARE[current.org.role] : "Your own account"}
            </small>
          </div>
          <nav aria-label="Pages">
            {items.map((item) => (
              <NavLink key={item.path} {...item} at={path} follow={follow} />
            ))}
          </nav>
        </aside>
      )}
      <main className="main">{children}</main>
      {items.length ? <Tabs items={items} at={path} follow={follow} /> : null}
    </div>
  );
}

function pagesOf(org: Org, billed: boolean): NavItem[] {
  return (Object.entries(ORG_PAGES) as [OrgPage, string][])
    .filter(([page]) => seesPage(org, page, billed))
    .map(([page, name]) => ({
      path: orgPath(org.id, page),
      name,
      icon: ORG_ICONS[page],
    }));
}

function NavLink({
  path,
  name,
  icon,
  count,
  at,
  follow,
  onFollow,
}: NavItem & { at: string; follow: Follow; onFollow?: () => void }) {
  return (
    <a
      className="nv"
      href={path}
      onClick={(event) => {
        follow(path)(event);
        onFollow?.();
      }}
      aria-current={path === at ? "page" : undefined}
    >
      {icon}
      <span>{name}</span>
      {count ? <i>{count}</i> : null}
    </a>
  );
}

function Scopes({
  at,
  orgs,
  team,
  canStart,
  offered,
  follow,
}: {
  at: string;
  orgs: Org[];
  team: TeamAccess;
  canStart: boolean;
  offered: boolean;
  follow: Follow;
}) {
  const strip = useRef<HTMLElement>(null);
  useEffect(() => {
    strip.current
      ?.querySelector("[aria-current]")
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [at]);
  const scope = (id: string, to: string, name: string, picture: ReactNode) => (
    <a
      key={id}
      className="scope"
      href={to}
      onClick={follow(to)}
      aria-current={id === at ? "true" : undefined}
      aria-label={name}
      title={name}
    >
      {picture}
    </a>
  );
  return (
    <nav className="scopes" aria-label="You and your orgs" ref={strip}>
      <a className="sk" href="/" onClick={follow("/")} aria-label="Sikemux">
        <img src={logo} alt="" />
      </a>
      {scope("you", "/", "You", <Avatar size={30} />)}
      {orgs.length > 0 || canStart ? <hr /> : null}
      {orgs.map((org) => {
        const still = !frozen(org)
          ? ""
          : renewable(org, team)
            ? "Its Team plan ended"
            : "Paused";
        return scope(
          org.id,
          orgPath(org.id),
          still ? `${org.name}. ${still}` : org.name,
          <>
            <OrgAvatar org={org} />
            {still ? <i className="dot" /> : null}
          </>,
        );
      })}
      {canStart
        ? scope(
            START,
            START,
            offered
              ? "Start or join an org. One matches your email domain"
              : "Start or join an org",
            <>
              <span className="plus">
                <IconPlus size={15} />
              </span>
              {offered ? <i className="dot new" /> : null}
            </>,
          )
        : null}
    </nav>
  );
}

const TABS = 5;

/** A phone's pages: along the bottom, those past the fourth behind More. */
function Tabs({
  items,
  at,
  follow,
}: {
  items: NavItem[];
  at: string;
  follow: Follow;
}) {
  const [more, setMore] = useState(false);
  const first = items.length > TABS ? items.slice(0, TABS - 1) : items;
  const rest = items.slice(first.length);
  return (
    <nav className="tabs" aria-label="Pages">
      {first.map((item) => (
        <a
          key={item.path}
          href={item.path}
          onClick={follow(item.path)}
          aria-current={item.path === at ? "page" : undefined}
        >
          {item.icon}
          <span>{item.name}</span>
        </a>
      ))}
      {rest.length ? (
        <button
          type="button"
          aria-haspopup="dialog"
          aria-current={rest.some((item) => item.path === at) || undefined}
          onClick={() => setMore(true)}
        >
          <IconMore size={15} />
          <span>More</span>
        </button>
      ) : null}
      {more ? (
        <Dialog title="More" onClose={() => setMore(false)}>
          <div className="sheet">
            {rest.map((item) => (
              <NavLink
                key={item.path}
                {...item}
                at={at}
                follow={follow}
                onFollow={() => setMore(false)}
              />
            ))}
          </div>
        </Dialog>
      ) : null}
    </nav>
  );
}
