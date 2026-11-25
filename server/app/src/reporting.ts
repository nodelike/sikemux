import { useAuth } from "@clerk/react";
import { externalValue, useExternalValue } from "@shared/lib/externalValue";
import { useEffect } from "react";
import type { ErrorInfo } from "react";
import type { RootOptions } from "react-dom/client";

import { isPagePath, isSharePath } from "./navigation.ts";

const dsn: string | undefined = import.meta.env.VITE_SENTRY_DSN;
const anonymousShare =
  isPagePath(location.pathname) || isSharePath(location.pathname);

function privacyOptOut(): boolean {
  const nav = navigator as Navigator & { globalPrivacyControl?: boolean };
  return nav.doNotTrack === "1" || nav.globalPrivacyControl === true;
}

/** Only the release CI deploys has a DSN, so dev servers and other builds have none and send nothing. */
export const reporting = dsn !== undefined && dsn !== "" && !anonymousShare;

const DECLINED_KEY = "sikemux.crashReports";
const shared = externalValue(
  !anonymousShare &&
    !privacyOptOut() &&
    localStorage.getItem(DECLINED_KEY) !== "off",
);

/** Sentry, loaded beside the page rather than before it. */
const load = (address: string) =>
  import("./sentry.ts").then((loaded) => {
    loaded.startSentry(address);
    loaded.send(shared.get());
    return loaded;
  });

let sentry = reporting && dsn && shared.get() ? load(dsn) : null;

export function useCrashReportsShared(): boolean {
  return useExternalValue(shared);
}

export function shareCrashReports(next: boolean) {
  shared.set(next);
  if (next) localStorage.removeItem(DECLINED_KEY);
  else localStorage.setItem(DECLINED_KEY, "off");
  if (!dsn || !reporting) return;
  if (next) sentry ??= load(dsn);
  void sentry?.then((loaded) => loaded.send(shared.get()));
}

const reportReactError = (error: unknown, info: ErrorInfo) =>
  void sentry?.then((loaded) => loaded.reportReactError(error, info));

/** React's own reports of what broke a render, sent with the components it broke in. */
export const rootOptions: RootOptions = reporting
  ? {
      onUncaughtError: reportReactError,
      onCaughtError: reportReactError,
      onRecoverableError: reportReactError,
    }
  : {};

/** Marks what is reported as the signed-in account's, by a pseudonym. */
export function ReportedAccount() {
  const { userId } = useAuth();
  const on = useCrashReportsShared();
  useEffect(() => {
    if (on) void sentry?.then((loaded) => loaded.setAccount(userId ?? null));
  }, [userId, on]);
  return null;
}
