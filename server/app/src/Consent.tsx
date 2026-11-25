import { useClerk, useOAuthConsent, useUser } from "@clerk/react";
import { IconLaptop, IconMail, IconUser } from "@shared/ui/Icons";
import { useState } from "react";

import { Avatar } from "./Avatar.tsx";
import { Gate, GateActions, GateFacts, GateWho, Loading } from "./Frame.tsx";
import { brandMark } from "./marks.ts";
import { maskEmail } from "./maskEmail.ts";
import { Btn, btnClass, Problem, Txt } from "./ui.tsx";

function scopeIcon(scope: string) {
  if (scope.includes("email")) return <IconMail size={16} />;
  if (scope.includes("profile") || scope.includes("openid"))
    return <IconUser size={16} />;
  return <IconLaptop size={16} />;
}

/** Where Clerk sends a sign-in an app started, such as the Mac's, to ask the person first. */
export function Consent() {
  const params = new URLSearchParams(location.search);
  const clientId = params.get("client_id") ?? "";
  const redirectUri = params.get("redirect_uri") ?? "";

  if (!clientId || !redirectUri)
    return (
      <Gate
        title="Nothing to sign in"
        lede="This page opens when Sikemux on a computer asks to sign in. Start from Settings, Devices in the app."
        mark={brandMark}
        scene="sign"
        tone="waiting"
      >
        <GateActions>
          <a className={btnClass({ quiet: true })} href="/">
            Go to your account
          </a>
        </GateActions>
      </Gate>
    );

  return (
    <ConsentAsk clientId={clientId} redirectUri={redirectUri} params={params} />
  );
}

function ConsentAsk({
  clientId,
  redirectUri,
  params,
}: {
  clientId: string;
  redirectUri: string;
  params: URLSearchParams;
}) {
  const clerk = useClerk();
  const { user } = useUser();
  const [answering, setAnswering] = useState<"allow" | "deny">();
  const { data, isLoading, error } = useOAuthConsent({
    oauthClientId: clientId,
    scope: params.get("scope") ?? undefined,
    redirectUri,
  });

  if (isLoading || (!data && !error)) return <Loading />;
  if (error || !data)
    return (
      <Gate
        title="This sign-in can't continue"
        lede="Start again from Sikemux on your computer."
        mark={brandMark}
        scene="sign"
        tone="failed"
      >
        <Problem>
          {error?.errors[0]?.longMessage ??
            "The request from the app is no longer valid."}
        </Problem>
        <GateActions>
          <a className={btnClass({ quiet: true })} href="/">
            Go to your account
          </a>
        </GateActions>
      </Gate>
    );

  const destination = destinationOf(redirectUri);
  const email = user?.primaryEmailAddress?.emailAddress;
  const name = user?.fullName || user?.username || "Your account";
  const scopes = data.scopes.filter((scope) => scope.requiresConsent);

  return (
    <Gate
      title={`Sign in to ${data.oauthApplicationName}?`}
      lede={`${data.oauthApplicationName} will be signed in to your Sikemux account and can see:`}
      mark={brandMark}
      scene="sign"
      tone="waiting"
      say={`You go back to ${destination} afterwards.`}
    >
      {scopes.length > 0 ? (
        <GateFacts
          items={scopes.map((scope) => ({
            key: scope.scope,
            icon: scopeIcon(scope.scope),
            title: scope.description ?? scope.scope,
          }))}
        />
      ) : null}
      <GateWho
        lead={<Avatar size={36} />}
        name={name}
        detail={email ? maskEmail(email) : undefined}
        action={
          <Txt
            disabled={answering !== undefined}
            onClick={() => void clerk.signOut({ redirectUrl: location.href })}
          >
            Not you?
          </Txt>
        }
      />
      <form
        method="POST"
        action={clerk.oauthApplication.buildConsentActionUrl({ clientId })}
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
        <GateActions>
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
        </GateActions>
      </form>
    </Gate>
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
