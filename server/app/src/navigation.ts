import { useEffect, useState, type MouseEvent } from "react";

import { config } from "./config.ts";

export const DELETE_ACCOUNT = "/delete-account";

const RETURN_KEY = "sikemux.returnTo";

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
    const consent =
      url.origin === location.origin && url.pathname === "/oauth-consent";
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

/** A shared chat's page, which loads on its own rather than inside the account pages. */
export function isSharePath(path: string): boolean {
  return path.startsWith("/s/");
}

/** The shared chat a sign-in started from, taken once, wherever the sign-in came back to. */
export function takeShareReturn(): string | null {
  const saved = sessionStorage.getItem(RETURN_KEY);
  if (!saved || !isSharePath(saved)) return null;
  sessionStorage.removeItem(RETURN_KEY);
  return saved;
}

export function takeReturn(): string {
  const saved = sessionStorage.getItem(RETURN_KEY) ?? "/";
  sessionStorage.removeItem(RETURN_KEY);
  if (isSharePath(saved)) return saved;
  const [path = "/", query = ""] = saved.split("?", 2);
  if (path === "/" && appReturn(`?${query}`)) return saved;
  if (path === "/oauth-consent" || path === "/host") return saved;
  return path === DELETE_ACCOUNT ? path : "/";
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
    setPath(to);
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
