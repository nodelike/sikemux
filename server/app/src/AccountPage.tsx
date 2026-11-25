import type { TeamAccess } from "@sikemux/protocol";
import { useClerk, useSession, useUser } from "@clerk/react";
import { formatDay } from "@shared/lib/format";
import { IconLogOut, IconMail, IconShield, IconTrash } from "@shared/ui/Icons";
import { useState, type MouseEvent } from "react";

import { personSvg } from "./avatarSvg.ts";
import { DELETE_ACCOUNT } from "./navigation.ts";
import {
  reporting,
  shareCrashReports,
  useCrashReportsShared,
} from "./reporting.ts";
import { PROVIDERS, SignInWaysCard, waysOf } from "./SignInWays.tsx";
import { Card, Column, Page, Row, Rows, Switch, Tile, Txt } from "./ui.tsx";
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

const COUNTED = ["No", "One", "Two", "Three", "Four", "Five", "Six"];

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
      title={user.fullName || user.username || "Your account"}
      lede={
        byCodeOnly
          ? "One way in: a code by email. Add another so you never get locked out."
          : ways.length === 1
            ? "One way to sign in. Add another so you never get locked out."
            : `${COUNTED[ways.length] ?? ways.length} ways to sign in, all to this account.`
      }
      mark={{ svg: personSvg(user.id) }}
      scene="you"
    >
      <SignInWaysCard />
      <Column w={5} side>
        <WorkAddressesCard team={team} />
        <ThisBrowser onDelete={onDelete} />
      </Column>
    </Page>
  );
}

function ThisBrowser({
  onDelete,
}: {
  onDelete: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { signOut } = useClerk();
  const { session } = useSession();
  const sharing = useCrashReportsShared();
  const [leaving, setLeaving] = useState(false);
  const leave = () => {
    setLeaving(true);
    signOut({ redirectUrl: "/" }).catch(() => setLeaving(false));
  };
  return (
    <Card label="This browser">
      <Rows>
        <Row
          lead={
            <Tile>
              <IconLogOut size={16} />
            </Tile>
          }
          name={
            session
              ? `Signed in since ${formatDay(session.createdAt.toISOString(), "short")}`
              : "Signed in"
          }
          end={
            <Txt onClick={leave} disabled={leaving}>
              {leaving ? "Signing out" : "Sign out"}
            </Txt>
          }
        />
        {reporting ? (
          <Row
            lead={
              <Tile>
                <IconShield size={16} />
              </Tile>
            }
            name="Share crash reports"
            detail="Errors and page speed, without what you typed."
            end={
              <Switch
                label="Share crash reports"
                on={sharing}
                onChange={shareCrashReports}
              />
            }
          />
        ) : null}
        <Row
          lead={
            <Tile>
              <IconTrash size={16} />
            </Tile>
          }
          name="Delete account"
          detail="Your hosts keep their files."
          end={
            <a className="txt bad" href={DELETE_ACCOUNT} onClick={onDelete}>
              Delete
            </a>
          }
        />
      </Rows>
    </Card>
  );
}
