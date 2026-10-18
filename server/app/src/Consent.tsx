import { useClerk, useOAuthConsent, useUser } from "@clerk/react";
import { useState, type ReactNode } from "react";

import { Avatar } from "./Avatar.tsx";
import { Logo } from "./icons.tsx";

export const CONSENT = "/oauth-consent";

/** Where Clerk sends a sign-in an app started, such as the Mac's, to ask the person first. */
export function Consent() {
  const clerk = useClerk();
  const { user } = useUser();
  const [answering, setAnswering] = useState<"allow" | "deny">();
  const params = new URLSearchParams(location.search);
  const clientId = params.get("client_id") ?? "";
  const redirectUri = params.get("redirect_uri") ?? "";
  const { data, isLoading, error } = useOAuthConsent({
    oauthClientId: clientId,
    scope: params.get("scope") ?? undefined,
    redirectUri,
    enabled: Boolean(clientId && redirectUri),
  });

  if (!clientId || !redirectUri)
    return (
      <Panel>
        <h1>Nothing to sign in</h1>
        <p className="lede">
          This page opens when Sikemux on a computer asks to sign in. Start from
          Settings, Devices in the app.
        </p>
      </Panel>
    );
  if (isLoading)
    return (
      <Panel>
        <p className="quiet">Loading…</p>
      </Panel>
    );
  if (error || !data)
    return (
      <Panel>
        <h1>This sign-in can't continue</h1>
        <p className="problem">
          {error?.errors[0]?.longMessage ??
            "The request from the app is no longer valid."}
        </p>
        <p className="lede">Start again from Sikemux on your computer.</p>
      </Panel>
    );

  const destination = destinationOf(redirectUri);
  const email = user?.primaryEmailAddress?.emailAddress;
  return (
    <Panel>
      <h1>Sign in to {data.oauthApplicationName}?</h1>
      <div className="consent-who">
        <Avatar size={32} />
        <span className="ink">{email}</span>
      </div>
      <p className="lede">
        {data.oauthApplicationName} will be signed in to your Sikemux account
        and can see:
      </p>
      <ul className="consent-scopes">
        {data.scopes
          .filter((scope) => scope.requiresConsent)
          .map((scope) => (
            <li key={scope.scope}>{scope.description ?? scope.scope}</li>
          ))}
      </ul>
      <p className="quiet consent-return">
        You go back to {destination} afterwards.
      </p>
      <form
        method="POST"
        action={clerk.oauthApplication.buildConsentActionUrl({ clientId })}
        className="consent-actions"
        onSubmit={(event) => {
          const submitter = (event.nativeEvent as SubmitEvent)
            .submitter as HTMLButtonElement | null;
          setAnswering(submitter?.value === "true" ? "allow" : "deny");
        }}
      >
        {[...params.entries()]
          .filter(([key]) => key !== "consented" && key !== "organization_id")
          .map(([key, value], index) => (
            <input
              key={`${key}:${index}`}
              type="hidden"
              name={key}
              value={value}
            />
          ))}
        <button
          type="submit"
          name="consented"
          value="false"
          className="button"
          data-busy={answering === "deny" || undefined}
        >
          Deny
        </button>
        <button
          type="submit"
          name="consented"
          value="true"
          className="button primary"
          data-busy={answering === "allow" || undefined}
        >
          Allow
        </button>
      </form>
    </Panel>
  );
}

export function Panel({ children }: { children: ReactNode }) {
  return (
    <section className="panel">
      <div className="brand">
        <Logo size={22} />
        <span>Sikemux</span>
      </div>
      {children}
    </section>
  );
}

/** The app on this computer listens on a loopback address; anywhere else is named by its host. */
function destinationOf(redirectUri: string): string {
  try {
    const host = new URL(redirectUri).hostname;
    return host === "127.0.0.1" || host === "localhost" || host === "[::1]"
      ? "Sikemux on this computer"
      : host;
  } catch {
    return redirectUri;
  }
}
