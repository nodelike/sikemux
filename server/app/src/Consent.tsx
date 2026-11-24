import { useClerk, useOAuthConsent, useUser } from "@clerk/react";
import { useState } from "react";

import { Avatar } from "./Avatar.tsx";
import { Foot, Loading } from "./Frame.tsx";
import { iconMark } from "./marks.ts";
import { Btn, Hero, Problem } from "./ui.tsx";

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
      <Hero
        title="Nothing to sign in"
        lede="This page opens when Sikemux on a computer asks to sign in. Start from Settings, Devices in the app."
        mark={iconMark("laptop")}
        scene="sign"
        tone="waiting"
        foot={<Foot />}
      />
    );
  if (isLoading || (!data && !error)) return <Loading />;
  if (error || !data)
    return (
      <Hero
        title="This sign-in can't continue"
        lede="Start again from Sikemux on your computer."
        mark={iconMark("laptop")}
        scene="sign"
        tone="failed"
        foot={<Foot />}
      >
        <div className="do">
          <Problem>
            {error?.errors[0]?.longMessage ??
              "The request from the app is no longer valid."}
          </Problem>
        </div>
      </Hero>
    );

  const destination = destinationOf(redirectUri);
  const email = user?.primaryEmailAddress?.emailAddress;
  return (
    <Hero
      title={`Sign in to ${data.oauthApplicationName}?`}
      lede={`${data.oauthApplicationName} will be signed in to your Sikemux account and can see:`}
      mark={iconMark("laptop")}
      scene="sign"
      tone={answering ? "waiting" : undefined}
      foot={<Foot />}
    >
      <ul className="facts">
        {data.scopes
          .filter((scope) => scope.requiresConsent)
          .map((scope) => (
            <li key={scope.scope}>{scope.description ?? scope.scope}</li>
          ))}
      </ul>
      <p className="who">
        <Avatar size={24} />
        {email}
      </p>
      <form
        method="POST"
        action={clerk.oauthApplication.buildConsentActionUrl({ clientId })}
        className="do"
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
        <Btn
          type="submit"
          name="consented"
          value="true"
          data-busy={answering === "allow" || undefined}
        >
          Allow
        </Btn>
        <Btn
          quiet
          type="submit"
          name="consented"
          value="false"
          data-busy={answering === "deny" || undefined}
        >
          Deny
        </Btn>
      </form>
      <p className="fine">You go back to {destination} afterwards.</p>
    </Hero>
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
