import type { TeamAccess } from "@sikemux/protocol";
import { useClerk, useSession, useUser } from "@clerk/react";
import { formatDay } from "@shared/lib/format";
import { IconMail } from "@shared/ui/Icons";
import { useState, type MouseEvent } from "react";

import "./account.css";
import { personSvg } from "./avatarSvg.ts";
import { DELETE_ACCOUNT } from "./navigation.ts";
import {
  reporting,
  shareCrashReports,
  useCrashReportsShared,
} from "./reporting.ts";
import { PROVIDERS, SignInWaysCard, waysOf } from "./SignInWays.tsx";
import {
  Btn,
  btnClass,
  Ending,
  Page,
  Row,
  Rows,
  Section,
  Switch,
} from "./ui.tsx";
import { WorkAddressesCard } from "./WorkAddresses.tsx";

/** The ways the signed-in account can sign in, as the person knows them. */
export function useSignInWays() {
  const { user } = useUser();
  const providers = (user?.externalAccounts ?? []).flatMap((account) => {
    const provider = PROVIDERS[account.provider];
    return provider ? [provider] : [];
  });
  return providers.length > 0
    ? providers
    : [{ name: "Email code", mark: <IconMail size={16} /> }];
}

export function AccountPage({
  team,
  onDelete,
}: {
  team: TeamAccess;
  onDelete: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { user } = useUser();
  if (!user) return null;
  const ways = waysOf(user);
  const byCodeOnly = ways.length === 1 && ways[0]?.kind === "email";

  return (
    <Page
      title="Account"
      lede={
        byCodeOnly
          ? "One way in: a code by email. Add another so you never get locked out."
          : ways.length === 1
            ? "One way to sign in. Add another so you never get locked out."
            : "How you sign in."
      }
      mark={{ svg: personSvg(user.id) }}
      scene="you"
    >
      <div className="acc-stack">
        <SignInWaysCard />
        <WorkAddressesCard team={team} />
        <ThisBrowser />
        <div className="acc-end">
          <Ending
            title="Delete your account"
            actions={
              <a
                className={btnClass({ bad: true })}
                href={DELETE_ACCOUNT}
                onClick={onDelete}
              >
                Delete account
              </a>
            }
          >
            Every device is signed out. Your hosts keep their files.
          </Ending>
        </div>
      </div>
    </Page>
  );
}

function ThisBrowser() {
  const { signOut } = useClerk();
  const { session } = useSession();
  const sharing = useCrashReportsShared();
  const [leaving, setLeaving] = useState(false);
  const leave = () => {
    setLeaving(true);
    signOut({ redirectUrl: "/" }).catch(() => setLeaving(false));
  };
  return (
    <Section label="This browser">
      <Rows>
        {reporting ? (
          <Row
            name="Send crash reports"
            detail="Errors and page speed, without what you typed."
            end={
              <Switch
                label="Send crash reports"
                on={sharing}
                onChange={shareCrashReports}
              />
            }
          />
        ) : null}
        <Row
          name="Sign out"
          detail={
            session
              ? `Signed in since ${formatDay(session.createdAt.toISOString(), "short")}. Of this browser only.`
              : "Of this browser only."
          }
          end={
            <Btn small quiet onClick={leave} disabled={leaving} busy={leaving}>
              {leaving ? "Signing out" : "Sign out"}
            </Btn>
          }
        />
      </Rows>
    </Section>
  );
}
