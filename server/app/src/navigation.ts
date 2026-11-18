import { useEffect, useState, type MouseEvent } from "react";

import { config } from "./config.ts";

export const DELETE_ACCOUNT = "/delete-account";
export const CONSENT = "/oauth-consent";
export const HOST = "/host";
export const HOSTS = "/hosts";
export const CONNECTORS = "/connectors";
export const BILLING = "/billing";
export const PRICING = "/pricing";
export const TEAM = "/team";
export const TEAMS = "/teams";
export const DOMAINS = "/domains";
export const AUDIT = "/audit-log";
export const POLICY = "/policy";
export const JOIN = "/orgs/join";
export const PAY = "/pay";
export const WORK_ADDRESS = "/work-address";

/** The signed-in pages a sign-in can come back to. */
const PAGES = new Set([
  "/",
  CONNECTORS,
  HOSTS,
  BILLING,
  TEAM,
  TEAMS,
  DOMAINS,
  AUDIT,
  POLICY,
  PRICING,
  DELETE_ACCOUNT,
]);

export const RETURN_KEY = "sikemux.returnTo";
export const SIGNED_UP_FOR_KEY = "sikemux.signedUpFor";

/** Clerk's own host, which the publishable key carries base64-encoded with a trailing `$`. */
const CLERK_HOST = atob(config.clerkPublishableKey.split("_")[2] ?? "").replace(
  /\$$/,
  "",
);

/**
 * Where Clerk asked to go back to after a sign-in started by an app, such as the Mac
 * signing in through the browser. Only Clerk's authorize page and this site's consent
 * page are followed.
 */
export function appReturn(search = location.search): string | null {
  const raw = new URLSearchParams(search).get("redirect_url");
  if (!raw) return null;
  try {
    const url = new URL(raw);
    const clerk =
      url.protocol === "https:" &&
      url.host === CLERK_HOST &&
      url.pathname.startsWith("/oauth/authorize");
    const consent = url.origin === location.origin && url.pathname === CONSENT;
    return clerk || consent ? url.href : null;
  } catch {
    return null;
  }
}

/** Lets a sign-in that leaves the page for Google or GitHub come back to where it started, a shared chat's key included. */
export function rememberReturn() {
  sessionStorage.setItem(
    RETURN_KEY,
    location.pathname + location.search + location.hash,
  );
}

/** Notes, before a sign-in, what the person came to the page for, so a new account can be asked whether it is their first. */
export function noteSignUpReason(path = location.pathname) {
  const hint: "share" | "invite" | null = isSharePath(path)
    ? "share"
    : path === JOIN && location.hash.length > 1
      ? "invite"
      : null;
  if (hint) sessionStorage.setItem(SIGNED_UP_FOR_KEY, hint);
}

/** A shared chat's page, which loads on its own rather than inside the account pages. */
export function isSharePath(path: string): boolean {
  return path.startsWith("/s/");
}

/** Where a sign-in that left the page should go back to, taken once, or null when none is under way. */
export function takeReturn(): string | null {
  const saved = sessionStorage.getItem(RETURN_KEY);
  if (saved === null) return null;
  sessionStorage.removeItem(RETURN_KEY);
  if (isSharePath(saved)) return saved;
  const [path = "/", query = ""] = saved.split("?", 2);
  if (path === "/" && appReturn(`?${query}`)) return saved;
  if (path === CONSENT || path === HOST || saved.startsWith(`${JOIN}#`))
    return saved;
  return PAGES.has(path) ? path : "/";
}

export function usePath() {
  const [path, setPath] = useState(location.pathname);

  useEffect(() => {
    const onPop = () => setPath(location.pathname);
    addEventListener("popstate", onPop);
    return () => removeEventListener("popstate", onPop);
  }, []);

  const go = (to: string, { replace = false } = {}) => {
    if (replace) history.replaceState(null, "", to);
    else history.pushState(null, "", to);
    setPath(location.pathname);
    scrollTo(0, 0);
  };

  /** An ordinary link that stays in the page, unless the person asked for a new tab. */
  const follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0)
      return;
    event.preventDefault();
    go(to);
  };

  return { path, go, follow };
}
