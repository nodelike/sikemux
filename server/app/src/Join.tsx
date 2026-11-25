import { useAuth, useClerk, useUser } from "@clerk/react";
import type { Org } from "@sikemux/protocol";
import { ApiProblem } from "@shared/account/api";
import { IconBack } from "@shared/ui/Icons";
import { type MouseEvent, useEffect, useEffectEvent, useState } from "react";

import { useSignInWays } from "./AccountPage.tsx";
import { problemText } from "./api.ts";
import { Avatar } from "./Avatar.tsx";
import { useClient } from "./Billing.tsx";
import { useCheer } from "./Field.tsx";
import { Foot, Loading } from "./Frame.tsx";
import { useInviteOrg } from "./invite.ts";
import { loadLogos, useOrgMark } from "./logos.ts";
import { iconMark } from "./marks.ts";
import { HOSTS, JOIN } from "./navigation.ts";
import { ROLE_WORDS } from "./orgs.ts";
import { Btn, Field, Hero, Problem, Row, Rows, Txt, useAction } from "./ui.tsx";

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;

export function JoinPage({
  canPaste,
  follow,
  onHome,
  onJoined,
}: {
  /** Whether someone without a link may paste one; unknown while the account loads. */
  canPaste?: boolean;
  follow: Follow;
  onHome: () => void;
  onJoined: (orgId: string) => Promise<void>;
}) {
  const client = useClient();
  const { getToken } = useAuth();
  const { user } = useUser();
  const { signOut } = useClerk();
  const signIns = useSignInWays();
  const cheer = useCheer();
  const [token, setToken] = useState(() => location.hash.slice(1));
  const orgName = useInviteOrg(token);
  const action = useAction();
  const opening = useAction();
  const [joined, setJoined] = useState<Org>();
  const joinedMark = useOrgMark(joined);

  useEffect(() => {
    if (location.hash) history.replaceState(null, "", location.pathname);
  }, []);

  const goHome = useEffectEvent(onHome);
  useEffect(() => {
    if (!token && canPaste === false) goHome();
  }, [token, canPaste]);

  if (joined)
    return (
      <Hero
        title={`You're in ${joined.name}`}
        lede={`You hold one of its seats as ${ROLE_WORDS[joined.role]}, so you have Pro on your own hosts while you hold it.`}
        mark={joinedMark}
        scene="blue"
        tone="worked"
        foot={<Foot />}
      >
        <div className="do">
          <Btn
            busy={opening.busy}
            onClick={() =>
              void opening.run(
                () => onJoined(joined.id),
                (error) =>
                  `${problemText(error)} Reload the page to try again.`,
              )
            }
          >
            Open {joined.name}
          </Btn>
        </div>
        <Problem>{opening.problem}</Problem>
      </Hero>
    );

  if (!token)
    return canPaste ? (
      <PasteLink follow={follow} onToken={setToken} />
    ) : (
      <Loading />
    );

  const name = user?.fullName || user?.username || "this account";
  const ways = new Intl.ListFormat("en", { type: "conjunction" }).format(
    signIns.map((way) => way.name),
  );
  return (
    <Hero
      title={orgName ? `Join ${orgName}` : "Join an org"}
      lede={`You've been sent an invite link. Joining takes one of ${
        orgName ? `${orgName}'s` : "the org's"
      } seats and gives you Pro on your own hosts while you hold it. Nobody in the org sees your hosts or your email address.`}
      mark={iconMark("link")}
      scene="blue"
      tone={action.problem ? "failed" : undefined}
      foot={<Foot />}
    >
      <Rows>
        <Row
          lead={<Avatar size={28} />}
          name={`Joining as ${name}`}
          detail={`The account on this browser. It signs in with ${ways}.`}
          wrap
          end={
            <Txt
              onClick={() => void signOut({ redirectUrl: `${JOIN}#${token}` })}
            >
              Not you?
            </Txt>
          }
        />
      </Rows>
      <div className="do">
        <Btn
          busy={action.busy}
          onClick={() =>
            void action.run(
              async () => {
                const org = await (
                  await client("join the org")
                ).joinOrg({ token });
                cheer();
                loadLogos([org], getToken);
                setJoined(org);
              },
              (error) =>
                error instanceof ApiProblem && error.status === 404
                  ? `${problemText(error)} Ask whoever sent the link for a new one.`
                  : problemText(error),
            )
          }
        >
          {orgName ? `Join ${orgName}` : "Join the org"}
        </Btn>
      </div>
      <Problem>{action.problem}</Problem>
      <p className="fine">
        The link works with any account. You don&rsquo;t need an address at{" "}
        {orgName ?? "the org"} to join.
      </p>
    </Hero>
  );
}

/** Where a person who has the link somewhere else puts it. */
function PasteLink({
  follow,
  onToken,
}: {
  follow: Follow;
  onToken: (token: string) => void;
}) {
  const [link, setLink] = useState("");
  const [problem, setProblem] = useState<string>();
  return (
    <Hero
      title="Join an org"
      lede="You can be in as many as you're invited to."
      mark={iconMark("link")}
      scene="blue"
      tone={problem ? "failed" : undefined}
      back={
        <a className="txt" href={HOSTS} onClick={follow(HOSTS)}>
          <IconBack className="ic" /> Back
        </a>
      }
      foot={<Foot />}
    >
      <form
        className="do"
        onSubmit={(event) => {
          event.preventDefault();
          const token = /#([\w-]{43})$/.exec(link.trim())?.[1];
          if (token) onToken(token);
          else
            setProblem(
              "That isn't a whole invite link. Copy all of it, including what follows the #.",
            );
        }}
      >
        <Field
          aria-label="Invite link"
          placeholder={`${location.host}${JOIN}#…`}
          value={link}
          autoFocus
          autoComplete="off"
          onChange={(event) => {
            setLink(event.target.value);
            setProblem(undefined);
          }}
        />
        <Btn type="submit" disabled={!link.trim()}>
          Continue
        </Btn>
      </form>
      <Problem>{problem}</Problem>
      <p className="fine">
        If a link brought you here, it has no invite in it. Ask the org&rsquo;s
        owner or an admin for a new link, and open the whole link.
      </p>
    </Hero>
  );
}
