import { useAuth, useClerk, useUser } from "@clerk/react";
import type { Org } from "@sikemux/protocol";
import { ApiProblem, connectApi } from "@shared/account/api";
import { IconArrowLeft } from "@shared/ui/Icons";
import { type MouseEvent, useEffect, useEffectEvent, useState } from "react";

import { useSignInWays } from "./AccountPage.tsx";
import { problemText } from "./api.ts";
import { Avatar } from "./Avatar.tsx";
import { useClient } from "./client.ts";
import { config } from "./config.ts";
import { useCheer } from "./Field.tsx";
import { Gate, GateActions, GateWho, Loading } from "./Frame.tsx";
import { loadLogos, useOrgMark } from "./logos.ts";
import { brandMark, iconMark } from "./marks.ts";
import { HOSTS, JOIN } from "./navigation.ts";
import { ROLE_WORDS } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { Btn, Problem, Txt, useAction } from "./ui.tsx";

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;

type InvitePreview =
  | { state: "unknown" }
  | { state: "ready"; orgName: string }
  | { state: "gone" };

function useInvitePreview(token: string) {
  const [preview, setPreview] = useState<InvitePreview>({ state: "unknown" });

  useEffect(() => {
    if (!token) return;
    let current = true;
    connectApi({
      url: config.apiUrl,
      offline: "Can't reach Sikemux.",
    })
      .previewOrgInvite({ token })
      .then(
        ({ orgName }) => {
          if (!current) return;
          setPreview(orgName ? { state: "ready", orgName } : { state: "gone" });
        },
        () => undefined,
      );
    return () => {
      current = false;
    };
  }, [token]);

  return { preview, markGone: () => setPreview({ state: "gone" }) };
}

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
  const { preview, markGone } = useInvitePreview(token);
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
      <Gate
        title={`You're in ${joined.name}`}
        lede={`You hold one of its seats as ${ROLE_WORDS[joined.role]}, so you have Pro on your own hosts while you hold it.`}
        mark={joinedMark}
        scene={orgScene(joined.id)}
        tone="worked"
      >
        <Problem>{opening.problem}</Problem>
        <GateActions>
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
        </GateActions>
      </Gate>
    );

  if (!token)
    return canPaste ? (
      <PasteLink follow={follow} onToken={setToken} />
    ) : (
      <Loading />
    );

  if (preview.state === "gone")
    return (
      <Gate
        title="This invite has ended"
        lede="It was already used, taken back, or is over 7 days old."
        mark={iconMark("link")}
        scene="sign"
        tone="failed"
        say="Ask whoever sent it for a new one."
      >
        <GateActions>
          <Btn quiet onClick={onHome}>
            Go to your account
          </Btn>
        </GateActions>
      </Gate>
    );

  const orgName = preview.state === "ready" ? preview.orgName : null;
  const name = user?.fullName || user?.username || "Your account";
  const ways = new Intl.ListFormat("en", { type: "conjunction" }).format(
    signIns.map((way) => way.name),
  );

  return (
    <Gate
      title={orgName ? `Join ${orgName}` : "Join an org"}
      lede={`You've been sent an invite link. Joining takes one of ${
        orgName ? `${orgName}'s` : "the org's"
      } seats and gives you Pro on your own hosts while you hold it.`}
      mark={brandMark}
      scene={orgName ? "blue" : "sign"}
      tone={action.problem ? "failed" : action.busy ? "waiting" : undefined}
      say="Nobody there sees your hosts or your email address. The invite works with any account."
    >
      <GateWho
        lead={<Avatar size={36} />}
        name={name}
        detail={`This browser's account. Signs in with ${ways}.`}
        action={
          <Txt
            disabled={action.busy}
            onClick={() => void signOut({ redirectUrl: `${JOIN}#${token}` })}
          >
            Not you?
          </Txt>
        }
      />
      <Problem>{action.problem}</Problem>
      <GateActions>
        <Btn
          busy={action.busy}
          onClick={() =>
            void action.run(
              async () => {
                try {
                  const org = await (
                    await client("join the org")
                  ).joinOrg({ token });
                  cheer();
                  loadLogos([org], getToken);
                  setJoined(org);
                } catch (error) {
                  if (error instanceof ApiProblem && error.status === 404) {
                    markGone();
                    return;
                  }
                  throw error;
                }
              },
              (error) => problemText(error),
            )
          }
        >
          {action.busy
            ? "Joining"
            : orgName
              ? `Join ${orgName}`
              : "Join the org"}
        </Btn>
        <Btn quiet disabled={action.busy} onClick={onHome}>
          Not now
        </Btn>
      </GateActions>
    </Gate>
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
    <Gate
      title="Join an org"
      lede="Paste the invite you were sent. You can be in as many orgs as you're invited to."
      mark={iconMark("link")}
      scene="sign"
      tone={problem ? "failed" : undefined}
      back={
        <a className="txt gback" href={HOSTS} onClick={follow(HOSTS)}>
          <IconArrowLeft size={14} />
          Back
        </a>
      }
      say="If a link brought you here, it has no invite in it. Ask the org's owner or an admin for a new one."
    >
      <form
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
        <label className={problem ? "gf bad" : "gf"}>
          <span>Invite link</span>
          <span className="gfi">
            <input
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
          </span>
        </label>
        <Problem>{problem}</Problem>
        <GateActions>
          <Btn type="submit" disabled={!link.trim()}>
            Continue
          </Btn>
        </GateActions>
      </form>
    </Gate>
  );
}
