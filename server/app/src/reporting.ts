import { useAuth } from "@clerk/react";
import { useEffect } from "react";
import type { ErrorInfo } from "react";
import type { RootOptions } from "react-dom/client";

const dsn: string | undefined = import.meta.env.VITE_SENTRY_DSN;

/**
 * Sentry, loaded beside the page rather than before it. Only the release CI deploys has a DSN,
 * so dev servers and other builds have none and send nothing.
 */
const sentry = dsn
  ? import("./sentry.ts").then((loaded) => {
      loaded.startSentry(dsn);
      return loaded;
    })
  : null;

export const reporting = sentry !== null;

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
  useEffect(() => {
    void sentry?.then((loaded) => loaded.setAccount(userId ?? null));
  }, [userId]);
  return null;
}
