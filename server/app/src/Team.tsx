import type {
  BillingCatalog,
  Org,
  OrgInvite,
  OrgMember,
  OrgRole,
  Subscription,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconCheck,
  IconCircleAlert,
  IconCopy,
  IconLink,
  IconPlus,
  IconUsers,
} from "@shared/ui/Icons";
import { useClerk, useUser } from "@clerk/react";
import { useEffect, useState, type MouseEvent, type ReactNode } from "react";

import { useSignInWays } from "./AccountPage.tsx";
import { problemText } from "./api.ts";
import { Avatar, PersonAvatar } from "./Avatar.tsx";
import {
  ChangeSeats,
  heldChange,
  NewTeam,
  NOT_OPEN,
  useCheckout,
  useClient,
} from "./Billing.tsx";
import {
  FEATURE_WORDS,
  featuresAdded,
  money,
  priceOf,
  productOf,
} from "./money.ts";
import { useInviteOrg } from "./invite.ts";
import { BILLING, JOIN, TEAM } from "./navigation.ts";
import {
  OfferRow,
  OrgChoice,
  ROLE_WORDS,
  useOffers,
  useOrgs,
} from "./OrgPages.tsx";
import {
  Confirm,
  ItemRow,
  PageHead,
  Row,
  Rows,
  Section,
  Segmented,
  useAction,
  useNotice,
} from "./ui.tsx";

/** Whether someone with `actor`'s role may change, remove or invite someone with `role`, as the API decides it. */
function manages(actor: OrgRole, role: OrgRole) {
  return actor === "owner" || (actor === "admin" && role !== "owner");
}

type Follow = (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;

type Detail = {
  members: OrgMember[];
  invites: OrgInvite[];
  team?: { sub: Subscription; catalog: BillingCatalog };
};

export function TeamPage({ follow }: { follow: Follow }) {
  const { orgs, org, problem, choose, reload, version } = useOrgs();
  const offers = useOffers(true);
  const { notice, show } = useNotice();

  const joined = (each: Org) => {
    show(`You joined ${each.name}`);
    choose(each.id);
    offers.reload();
    reload();
  };
  const offered = offers.offers.length ? (
    <Section label="Orgs at your email's domain">
      <Rows>
        {offers.offers.map((offer) => (
          <OfferRow key={offer.orgId} offer={offer} onJoined={joined} />
        ))}
      </Rows>
    </Section>
  ) : null;

  if (problem)
    return (
      <>
        <PageHead title="Team" />
        <p className="problem" role="alert">
          {problem}
        </p>
      </>
    );
  if (!orgs)
    return (
      <>
        <PageHead title="Team" />
        <div className="groups">
          <Section label="Seats">
            <Rows busy />
          </Section>
        </div>
      </>
    );
  if (!org)
    return (
      <>
        <NoOrg offered={offered} />
        {notice}
      </>
    );

  return (
    <>
      <PageHead title="Team">
        {org.name}. Everyone with a seat has Pro on their own hosts.
      </PageHead>
      <OrgChoice orgs={orgs} org={org} choose={choose} />
      <OrgView
        key={org.id}
        org={org}
        version={version}
        follow={follow}
        reload={reload}
        show={show}
        offered={offered}
      />
      {notice}
    </>
  );
}

function OrgView({
  org,
  version,
  follow,
  reload,
  show,
  offered,
}: {
  org: Org;
  version: number;
  follow: Follow;
  reload: () => void;
  show: (text: string) => void;
  offered: ReactNode;
}) {
  const client = useClient();
  const [detail, setDetail] = useState<Detail>();
  const [problem, setProblem] = useState<string>();
  const runs = org.role === "owner" || org.role === "admin";

  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const routes = await client("see your org");
        const [{ members }, { invites }] = await Promise.all([
          routes.listOrgMembers(org.id),
          runs
            ? routes.listOrgInvites(org.id)
            : Promise.resolve({ invites: [] }),
        ]);
        const team =
          org.role === "owner"
            ? await Promise.all([
                routes.getBillingCatalog(),
                routes.getBilling({ org: org.id }),
              ]).then(
                ([catalog, overview]) => {
                  const sub = overview.subscriptions.find(
                    (each) =>
                      each.product === "team" && each.status !== "ended",
                  );
                  return sub && overview.canBuy && !sub.refundRequestedAt
                    ? { sub, catalog }
                    : undefined;
                },
                () => undefined,
              )
            : undefined;
        if (current) setDetail({ members, invites, team });
      } catch (error) {
        if (current)
          setProblem(`${problemText(error)} Reload the page to try again.`);
      }
    })();
    return () => {
      current = false;
    };
  }, [client, org.id, org.role, runs, version]);

  if (problem)
    return (
      <p className="problem" role="alert">
        {problem}
      </p>
    );

  const frozen = org.seats.limit === 0;
  const free = Math.max(0, org.seats.limit - org.seats.held);

  return (
    <div className="groups">
      {frozen ? (
        <div className="banner warn" role="alert">
          <IconCircleAlert size={16} />
          <div className="grow">
            <b>{org.name} has no Team plan running.</b>
            <br />
            Invites and org features are frozen. The org's data and every
            member's own hosts are kept.
          </div>
          {org.role === "owner" ? (
            <a
              className="button small primary"
              href={BILLING}
              onClick={follow(BILLING)}
            >
              Renew Team
            </a>
          ) : null}
        </div>
      ) : null}
      <Seats
        org={org}
        free={free}
        team={detail?.team}
        onChanged={(text) => {
          show(text);
          reload();
        }}
      />
      <Section label="Members" count={detail?.members.length}>
        {detail ? (
          <Rows>
            {detail.members.map((member) => (
              <MemberRow
                key={member.id}
                org={org}
                member={member}
                onChanged={(text) => {
                  show(text);
                  reload();
                }}
              />
            ))}
          </Rows>
        ) : (
          <Rows busy />
        )}
      </Section>
      {runs ? (
        <Invites
          org={org}
          frozen={frozen}
          free={free}
          invites={detail?.invites}
          onChanged={(text) => {
            show(text);
            reload();
          }}
        />
      ) : null}
      {offered}
      <Leave org={org} onLeft={reload} show={show} />
    </div>
  );
}

function Seats({
  org,
  free,
  team,
  onChanged,
}: {
  org: Org;
  free: number;
  team?: Detail["team"];
  onChanged: (text: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const { held, limit } = org.seats;
  const max = team?.catalog.products.find(
    (product) => product.id === "team",
  )?.maxQuantity;
  const atRenewal = team ? heldChange(team.sub) : undefined;
  return (
    <Section label="Seats">
      <Rows>
        <Row
          label="Seats"
          action={
            team && !adding && !atRenewal ? (
              <button
                type="button"
                className="button small"
                onClick={() => setAdding(true)}
              >
                Add seats
              </button>
            ) : null
          }
          below={
            team && adding ? (
              <ChangeSeats
                sub={team.sub}
                name={org.name}
                min={team.sub.quantity + 1}
                max={max}
                onClose={() => setAdding(false)}
                onChanged={onChanged}
              />
            ) : null
          }
        >
          {limit > 0 ? (
            <>
              <Meter held={held} limit={limit} />
              <span>
                {held} of {limit} used
              </span>
              <span className="quiet">· {free} free</span>
              {atRenewal && team && atRenewal.quantity !== team.sub.quantity ? (
                <span className="quiet">
                  · {team.sub.quantity} paid until{" "}
                  {formatDay(team.sub.currentPeriodEnd, "long")}
                </span>
              ) : null}
            </>
          ) : (
            <span className="quiet">
              {held} {held === 1 ? "person" : "people"}, no seats paid for
            </span>
          )}
        </Row>
      </Rows>
    </Section>
  );
}

function Meter({ held, limit }: { held: number; limit: number }) {
  return (
    <span
      className="meter"
      role="meter"
      aria-valuemin={0}
      aria-valuemax={limit}
      aria-valuenow={held}
      aria-label="Seats used"
    >
      <span style={{ width: `${Math.min(100, (held / limit) * 100)}%` }} />
    </span>
  );
}

function MemberRow({
  org,
  member,
  onChanged,
}: {
  org: Org;
  member: OrgMember;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [open, setOpen] = useState<"role" | "remove">();
  const [role, setRole] = useState(member.role);
  const name = member.name ?? "Someone";
  const canManage = !member.you && manages(org.role, member.role);
  const roles = (Object.keys(ROLE_WORDS) as OrgRole[]).filter((each) =>
    manages(org.role, each),
  );

  return (
    <ItemRow
      glyph={
        <PersonAvatar name={member.name} imageUrl={member.imageUrl} size={22} />
      }
      name={
        <>
          {name}
          {member.you ? <span className="chip">You</span> : null}
        </>
      }
      detail={
        canManage
          ? `${ROLE_WORDS[member.role]} · joined ${formatDay(member.joinedAt)}`
          : `Joined ${formatDay(member.joinedAt)}`
      }
      end={
        canManage && !open ? (
          <>
            <button
              type="button"
              className="button small"
              onClick={() => setOpen("role")}
            >
              Role
            </button>
            <button
              type="button"
              className="button small quiet-danger"
              onClick={() => setOpen("remove")}
            >
              Remove
            </button>
          </>
        ) : !canManage ? (
          <span className="chip">{ROLE_WORDS[member.role]}</span>
        ) : null
      }
    >
      {open === "role" ? (
        <Confirm
          busy={action.busy}
          problem={action.problem}
          label={`Make ${ROLE_WORDS[role]}`}
          unchanged={role === member.role}
          onCancel={() => {
            action.clear();
            setRole(member.role);
            setOpen(undefined);
          }}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("change a role")
                ).setOrgMemberRole(org.id, member.id, { role });
                setOpen(undefined);
                onChanged(`${name} is now ${ROLE_WORDS[role]}`);
              },
              (error) => `${problemText(error)} Their role is unchanged.`,
            )
          }
        >
          <Segmented
            label="Role"
            value={role}
            onChange={setRole}
            options={roles.map((each) => ({
              value: each,
              name: ROLE_WORDS[each],
            }))}
          />
        </Confirm>
      ) : open === "remove" ? (
        <Confirm
          danger
          busy={action.busy}
          problem={action.problem}
          label={`Remove ${name}`}
          onCancel={() => {
            action.clear();
            setOpen(undefined);
          }}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("remove someone")
                ).removeOrgMember(org.id, member.id);
                setOpen(undefined);
                onChanged(`${name} was removed. Their seat is free.`);
              },
              (error) => `${problemText(error)} They are still in the org.`,
            )
          }
        >
          <p>
            {name} leaves {org.name} and loses the Pro their seat gave them.
            Their own hosts stay theirs.
          </p>
        </Confirm>
      ) : null}
    </ItemRow>
  );
}

function Invites({
  org,
  frozen,
  free,
  invites,
  onChanged,
}: {
  org: Org;
  frozen: boolean;
  free: number;
  invites?: OrgInvite[];
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const roles = (Object.keys(ROLE_WORDS) as OrgRole[]).filter((each) =>
    manages(org.role, each),
  );
  const [role, setRole] = useState<OrgRole>("user");
  const [made, setMade] = useState<Made>();

  const create = () =>
    void action.run(
      async () => {
        const created = await (
          await client("make an invite link")
        ).createOrgInvite(org.id, { role });
        const copied = await copy(created.link);
        setMade({ id: created.invite.id, link: created.link, copied });
        onChanged(
          copied
            ? "Invite link copied"
            : "Invite link made. Copy it before you leave the page.",
        );
      },
      (error) => `${problemText(error)} No link was made.`,
    );

  const blocked = frozen
    ? "Frozen until Team is renewed"
    : free === 0
      ? "Every seat is taken. Add seats to invite someone."
      : undefined;

  return (
    <Section label="Invite links" count={invites?.length}>
      {invites ? (
        <Rows>
          {invites.map((invite) => (
            <InviteRow
              key={invite.id}
              org={org}
              invite={invite}
              made={invite.id === made?.id ? made : undefined}
              onChanged={onChanged}
            />
          ))}
          <ItemRow
            glyph={<IconPlus size={16} />}
            name="New invite link"
            detail={
              blocked ??
              "Nothing is mailed: you send the link. It holds a free seat for 7 days."
            }
            end={
              <>
                {roles.length > 1 ? (
                  <Segmented
                    label="Joins as"
                    value={role}
                    onChange={setRole}
                    options={roles.map((each) => ({
                      value: each,
                      name: ROLE_WORDS[each],
                    }))}
                  />
                ) : null}
                <button
                  type="button"
                  className="button small"
                  onClick={create}
                  disabled={!!blocked || action.busy}
                  data-busy={action.busy || undefined}
                >
                  Create
                </button>
              </>
            }
          >
            {action.problem ? (
              <p className="problem" role="alert">
                {action.problem}
              </p>
            ) : null}
          </ItemRow>
        </Rows>
      ) : (
        <Rows busy />
      )}
    </Section>
  );
}

type Made = { id: string; link: string; copied: boolean };

async function copy(link: string) {
  return navigator.clipboard.writeText(link).then(
    () => true,
    () => false,
  );
}

function InviteRow({
  org,
  invite,
  made,
  onChanged,
}: {
  org: Org;
  invite: OrgInvite;
  made?: Made;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [copied, setCopied] = useState(made?.copied ?? false);
  return (
    <ItemRow
      glyph={<IconLink size={16} />}
      name={
        made ? (
          <code className="invite-link">{made.link}</code>
        ) : (
          `Invite link, made ${formatDay(invite.createdAt)}`
        )
      }
      detail={
        made
          ? `Shown only now. Send it to the person you're inviting. Joins as ${ROLE_WORDS[invite.role]}.`
          : `Single use · joins as ${ROLE_WORDS[invite.role]} · expires ${formatDay(invite.expiresAt)}`
      }
      end={
        <>
          {made ? (
            <button
              type="button"
              className="button small"
              onClick={() => void copy(made.link).then(setCopied)}
            >
              <IconCopy size={13} />
              {copied ? "Copied" : "Copy"}
            </button>
          ) : null}
          {manages(org.role, invite.role) ? (
            <button
              type="button"
              className="button small"
              disabled={action.busy}
              data-busy={action.busy || undefined}
              onClick={() =>
                void action.run(
                  async () => {
                    await (
                      await client("revoke an invite")
                    ).revokeOrgInvite(org.id, invite.id);
                    onChanged("Invite link revoked. Its seat is free.");
                  },
                  (error) => `${problemText(error)} The link still works.`,
                )
              }
            >
              Revoke
            </button>
          ) : null}
        </>
      }
    >
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
    </ItemRow>
  );
}

function Leave({
  org,
  onLeft,
  show,
}: {
  org: Org;
  onLeft: () => void;
  show: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [asking, setAsking] = useState(false);
  return (
    <Section label="Leave">
      <Rows>
        <Row
          label={`Leave ${org.name}`}
          action={
            asking ? null : (
              <button
                type="button"
                className="button small quiet-danger"
                onClick={() => setAsking(true)}
              >
                Leave org
              </button>
            )
          }
          below={
            asking ? (
              <Confirm
                danger
                busy={action.busy}
                problem={action.problem}
                label={`Leave ${org.name}`}
                onCancel={() => {
                  action.clear();
                  setAsking(false);
                }}
                onConfirm={() =>
                  void action.run(
                    async () => {
                      await (await client("leave the org")).leaveOrg(org.id);
                      show(`You left ${org.name}`);
                      onLeft();
                    },
                    (error) => `${problemText(error)} You are still in it.`,
                  )
                }
              >
                <p>
                  {org.role === "owner"
                    ? "You give up its seat at once. If you pay for its Team plan, the plan ends at the close of this period."
                    : "You give up its seat, and the Pro it gives you, at once."}
                </p>
              </Confirm>
            ) : null
          }
        >
          <span className="quiet">Your hosts and account stay yours.</span>
        </Row>
      </Rows>
    </Section>
  );
}

function NoOrg({ offered }: { offered: ReactNode }) {
  const client = useClient();
  const checkout = useCheckout();
  const [catalog, setCatalog] = useState<BillingCatalog>();
  const [canBuy, setCanBuy] = useState(false);
  const [naming, setNaming] = useState(false);

  useEffect(() => {
    let current = true;
    void client("see Team")
      .then((routes) =>
        Promise.all([routes.getBillingCatalog(), routes.getBilling()]),
      )
      .then(
        ([catalog, overview]) => {
          if (!current) return;
          setCatalog(catalog);
          setCanBuy(overview.canBuy);
        },
        () => undefined,
      );
    return () => {
      current = false;
    };
  }, [client]);

  const { added } = catalog ? featuresAdded(catalog, "team") : { added: [] };
  const unit = catalog
    ? priceOf(productOf(catalog, "team"), "month")
    : undefined;

  return (
    <>
      <PageHead title="Team">
        An org of your own, with seats, roles and shared hosts.
      </PageHead>
      <div className="groups">
        <Section label="No org yet">
          <div className="team-pitch">
            <span className="team-pitch-glyph">
              <IconUsers size={18} />
            </span>
            <p>
              An org comes with a Team plan
              {unit ? `, ${money(unit, { cents: false })} a seat a month` : ""}.
              Everything in Pro, plus:
            </p>
            <ul className="features">
              {added.map((feature) => (
                <li key={feature}>
                  <IconCheck size={13} />
                  <span>{FEATURE_WORDS[feature]}</span>
                </li>
              ))}
            </ul>
            {catalog && naming ? (
              <NewTeam
                catalog={catalog}
                checkout={checkout}
                onCancel={() => setNaming(false)}
              />
            ) : (
              <button
                type="button"
                className="button primary"
                disabled={!catalog || !canBuy}
                onClick={() => {
                  checkout.clear();
                  setNaming(true);
                }}
              >
                {canBuy || !catalog ? "Start Team" : NOT_OPEN}
              </button>
            )}
          </div>
        </Section>
        {offered}
        <Section label="Joining an org">
          <Rows>
            <ItemRow
              glyph={<IconLink size={16} />}
              name="Been given an invite link?"
              detail="Open it while signed in here and you join that org. Nothing is mailed, so ask the owner for one."
            />
          </Rows>
        </Section>
      </div>
    </>
  );
}

export function JoinPage({ follow }: { follow: Follow }) {
  const client = useClient();
  const { user } = useUser();
  const { signOut } = useClerk();
  const signIns = useSignInWays();
  const [token] = useState(() => location.hash.slice(1));
  const orgName = useInviteOrg(token);
  const action = useAction();
  const [joined, setJoined] = useState<Org>();

  useEffect(() => {
    if (location.hash) history.replaceState(null, "", location.pathname);
  }, []);

  const name = user?.fullName || user?.username || "this account";
  const ways = new Intl.ListFormat("en", { type: "conjunction" }).format(
    signIns.map((way) => way.name),
  );

  const body: ReactNode = joined ? (
    <>
      <PageHead title={`You're in ${joined.name}`}>
        You hold one of its seats as {ROLE_WORDS[joined.role]}, so you have Pro
        on your own hosts while you hold it.
      </PageHead>
      <a className="button primary" href={TEAM} onClick={follow(TEAM)}>
        Open Team
      </a>
    </>
  ) : !token ? (
    <>
      <PageHead title="Join an org">
        This link has no invite in it. Ask the org's owner or an admin for a new
        link, and open the whole link.
      </PageHead>
    </>
  ) : (
    <>
      <PageHead title={orgName ? `Join ${orgName}` : "Join an org"}>
        You've been sent an invite link. Joining takes one of{" "}
        {orgName ? `${orgName}'s` : "the org's"} seats and gives you Pro on your
        own hosts while you hold it. Nobody in the org sees your hosts or your
        email address.
      </PageHead>
      <Rows>
        <ItemRow
          glyph={<Avatar size={28} />}
          name={`Joining as ${name}`}
          detail={`The account on this browser. It signs in with ${ways}.`}
          end={
            <button
              type="button"
              className="remove"
              onClick={() => void signOut({ redirectUrl: `${JOIN}#${token}` })}
            >
              Not you?
            </button>
          }
        />
      </Rows>
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
      <button
        type="button"
        className="button primary"
        disabled={action.busy}
        data-busy={action.busy || undefined}
        onClick={() =>
          void action.run(
            async () => {
              setJoined(
                await (await client("join the org")).joinOrg({ token }),
              );
            },
            (error) =>
              `${problemText(error)} Ask whoever sent the link for a new one.`,
          )
        }
      >
        {orgName ? `Join ${orgName}` : "Join the org"}
      </button>
      <p className="fine">
        The link works with any account. You don&rsquo;t need an address at{" "}
        {orgName ?? "the org"} to join.
      </p>
    </>
  );

  return <div className="join">{body}</div>;
}
