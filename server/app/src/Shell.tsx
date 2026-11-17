import { useClerk, useUser } from "@clerk/react";
import { IconLogOut } from "@shared/ui/Icons";
import { useState, type MouseEvent, type ReactNode } from "react";

import { Avatar } from "./Avatar.tsx";
import { Logo } from "./icons.tsx";

export type NavItem = {
  path: string;
  name: string;
  icon: ReactNode;
  count?: string;
};

export type NavGroup = { label?: string; items: NavItem[] };

/** The signed-in pages: a rail of sections beside one column, or a strip across the top on a phone. */
export function Shell({
  path,
  follow,
  groups,
  meta,
  wide,
  children,
}: {
  path: string;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  groups: NavGroup[];
  meta?: string;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="shell">
      <nav className="rail" aria-label="Account">
        <div className="brand">
          <Logo size={18} />
          <span>Sikemux</span>
        </div>
        <div className="rail-items">
          {groups.map((group, index) => (
            <div className="rail-group" key={group.label ?? index}>
              {group.label ? (
                <div className="rail-label">{group.label}</div>
              ) : null}
              {group.items.map((item) => (
                <a
                  key={item.path}
                  className="nav"
                  href={item.path}
                  onClick={follow(item.path)}
                  aria-current={item.path === path ? "page" : undefined}
                >
                  {item.icon}
                  <span>{item.name}</span>
                  {item.count ? (
                    <span className="nav-count">{item.count}</span>
                  ) : null}
                </a>
              ))}
            </div>
          ))}
        </div>
        <Me meta={meta} />
      </nav>
      <main className="main">
        <div className="column" data-wide={wide || undefined}>
          {children}
        </div>
      </main>
    </div>
  );
}

function Me({ meta }: { meta?: string }) {
  const { user } = useUser();
  const { signOut } = useClerk();
  const [leaving, setLeaving] = useState(false);
  const name = user?.fullName || user?.username || "Your account";

  const leave = () => {
    setLeaving(true);
    signOut({ redirectUrl: "/" }).catch(() => setLeaving(false));
  };

  return (
    <div className="me">
      <Avatar />
      <div className="me-text">
        <div className="me-name">{name}</div>
        {meta ? <div className="me-meta">{meta}</div> : null}
      </div>
      <button
        type="button"
        className="sign-out"
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
