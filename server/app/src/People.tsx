import type {
  Org,
  OrgInvite,
  OrgInviteCreated,
  OrgMember,
  OrgRole,
  OrgTeam,
  TeamAccess,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconCopy,
  IconExpandAll,
  IconLink,
  IconLogOut,
  IconMore,
  IconPlus,
  IconStack,
} from "@shared/ui/Icons";
import { useState, type MouseEvent } from "react";

import { problemText } from "./api.ts";
import { memberSvg, orgSvg } from "./avatarSvg.ts";
import { useClient } from "./Billing.tsx";
import { useCheer } from "./Field.tsx";
import { iconMark } from "./marks.ts";
import { orgPath } from "./navigation.ts";
import {
  changeable,
  frozen,
  manages,
  renewable,
  ROLE_WORDS,
  runs,
  useLoad,
} from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { freeSeats, MemberAvatar, people, Seats } from "./Seats.tsx";
import {
  Btn,
  btnClass,
  Card,
  Chip,
  Column,
  Confirm,
  Empty,
  Field,
  Page,
  Popover,
  Problem,
  Row,
  Rows,
  Seg,
  Switch,
  Tile,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

const ROLES: OrgRole[] = ["owner", "admin", "user"];

const ROLE_DETAIL: Record<OrgRole, string> = {
  owner: "Everything, with the bill",
  admin: "People, domains and policy",
  user: "Works in the org",
};

const nameOf = (member: OrgMember) => member.name ?? "Someone";

const copyText = (text: string) =>
  navigator.clipboard.writeText(text).then(
    () => true,
    () => false,
  );

export function PeoplePage({
  org,
  team,
  version,
  follow,
  reload,
  onLeft,
}: {
  org: Org;
  team: TeamAccess;
  version: number;
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  reload: () => Promise<void>;
  onLeft: () => Promise<void>;
}) {
  const client = useClient();
  const say = useToast();
  const [inviting, setInviting] = useState(false);
  const [made, setMade] = useState<OrgInviteCreated>();
  const loaded = useLoad(async () => {
    const routes = await client("see your org");
    const [{ members }, { invites }, { teams }] = await Promise.all([
      routes.listOrgMembers(org.id),
      runs(org)
        ? routes.listOrgInvites(org.id)
        : Promise.resolve({ invites: [] }),
      routes.listOrgTeams(org.id),
    ]);
    return { members, invites, teams };
  }, `${version}:${org.role}`);

  const changed = (text: string, undo?: () => Promise<void>) => {
    say(text, undo);
    loaded.again();
    void reload();
  };

  const scene = orgScene(org.id);
  const mark = { svg: orgSvg(org.id) };

  if (loaded.problem)
    return (
      <Page
        title="People didn't load"
        lede="Nothing in the org has changed."
        actions={<Btn onClick={loaded.again}>Try again</Btn>}
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Card label="People">
          <Problem>{loaded.problem}</Problem>
        </Card>
      </Page>
    );

  const { members, invites, teams } = loaded.data ?? {};
  const free = freeSeats(org);
  const boss = members?.find((each) => each.role === "owner");
  const bossName = boss?.name ?? "An owner";
  const open = changeable(org, team);
  const ended = renewable(org, team);
  const billing = orgPath(org.id, "billing");

  const body = members && invites && teams && (
    <>
      <Card label="People" count={members.length} w={7}>
        <Rows>
          {members.map((member) => (
            <MemberRow
              key={member.id}
              org={org}
              member={member}
              groups={teams
                .filter((each) => each.members.includes(member.id))
                .map((each) => each.name)}
              onChanged={changed}
            />
          ))}
        </Rows>
      </Card>
      <Column w={5} side>
        <Seats org={org} members={members} />
        {runs(org) && (open || invites.length > 0) ? (
          <Invites
            org={org}
            invites={invites}
            made={made}
            onChanged={changed}
          />
        ) : null}
        <Groups
          org={org}
          members={members}
          teams={teams}
          editable={runs(org) && open}
          onChanged={changed}
        />
        <Leave org={org} members={members} onLeft={onLeft} />
      </Column>
    </>
  );

  if (inviting && open)
    return (
      <InvitePage
        org={org}
        made={made}
        onMade={(created) => {
          setMade(created);
          loaded.again();
          void reload();
        }}
        onAnother={() => setMade(undefined)}
        onClose={() => setInviting(false)}
      >
        {body}
      </InvitePage>
    );

  if (frozen(org))
    return (
      <Page
        title={`${org.name} is ${ended ? "frozen" : "paused"}`}
        lede={
          ended
            ? "Its Team plan ended. Nothing is deleted."
            : "Team isn't open to your account now. Nothing is deleted."
        }
        actions={
          !ended ? undefined : org.role === "owner" ? (
            <a
              className={btnClass({})}
              href={billing}
              onClick={follow(billing)}
            >
              Renew Team
            </a>
          ) : (
            <p className="note">{bossName} can renew it.</p>
          )
        }
        mark={mark}
        scene={scene}
        tone="waiting"
      >
        {body ?? <Loading />}
      </Page>
    );

  return (
    <Page
      title={org.name}
      lede={
        !members
          ? "Loading its people…"
          : free === 0
            ? "Every seat is taken."
            : members.length === 1
              ? `Only you so far. ${free} ${free === 1 ? "seat is" : "seats are"} waiting.`
              : `${people(members.length)} here. ${free} of ${org.seats.limit} seats are free.`
      }
      actions={
        !members ? undefined : !open ? (
          <p className="note">
            Team isn't open to your account now. You can still read, hand over
            and leave.
          </p>
        ) : !runs(org) ? (
          <p className="note">
            Ask {bossName} for an invite link to bring someone in.
          </p>
        ) : free > 0 ? (
          <Btn
            onClick={() => {
              setMade(undefined);
              setInviting(true);
            }}
          >
            Invite someone
          </Btn>
        ) : org.preview ? (
          <p className="note">
            The preview holds {org.seats.limit}. Revoke a link or remove someone
            to free a seat.
          </p>
        ) : org.role === "owner" ? (
          <a className={btnClass({})} href={billing} onClick={follow(billing)}>
            Add seats
          </a>
        ) : (
          <p className="note">{bossName} can add seats.</p>
        )
      }
      mark={mark}
      scene={scene}
    >
      {body ?? <Loading />}
    </Page>
  );
}

const Loading = () => (
  <>
    <Card label="People" w={7} busy />
    <Column w={5} side>
      <Card label="Seats" busy />
    </Column>
  </>
);

function InvitePage({
  org,
  made,
  onMade,
  onAnother,
  onClose,
  children,
}: {
  org: Org;
  made?: OrgInviteCreated;
  onMade: (created: OrgInviteCreated) => void;
  onAnother: () => void;
  onClose: () => void;
  children: React.ReactNode;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const roles = ROLES.filter((each) => manages(org.role, each)).reverse();
  const [role, setRole] = useState<OrgRole>("user");
  const [copied, setCopied] = useState(false);

  if (made)
    return (
      <Page
        title="Link ready"
        lede="Send it yourself. It works once, for 7 days."
        actions={
          <div className="form">
            <code className="linkbox mono">{made.link}</code>
            <Btn onClick={() => void copyText(made.link).then(setCopied)}>
              <IconCopy size={14} />
              {copied ? "Copied" : "Copy link"}
            </Btn>
            <Btn
              quiet
              onClick={() => {
                setCopied(false);
                onAnother();
              }}
            >
              Make another
            </Btn>
            <Btn quiet onClick={onClose}>
              Done
            </Btn>
          </div>
        }
        mark={iconMark("link")}
        scene={orgScene(org.id)}
      >
        {children}
      </Page>
    );

  return (
    <Page
      title={`Invite to ${org.name}`}
      lede="They join as"
      actions={
        <>
          <div className="form">
            <Seg
              label="Joins as"
              value={role}
              disabled={action.busy}
              onChange={setRole}
              options={roles.map((each) => ({
                value: each,
                name: ROLE_WORDS[each],
              }))}
            />
            <Btn
              busy={action.busy}
              onClick={() =>
                void action.run(
                  async () => {
                    const created = await (
                      await client("make an invite link")
                    ).createOrgInvite(org.id, { role });
                    cheer();
                    onMade(created);
                  },
                  (error) => `${problemText(error)} No link was made.`,
                )
              }
            >
              Make a link
            </Btn>
            <Btn quiet disabled={action.busy} onClick={onClose}>
              Back
            </Btn>
          </div>
          <Problem>{action.problem}</Problem>
        </>
      }
      mark={iconMark("link")}
      scene={orgScene(org.id)}
      tone={action.problem ? "failed" : undefined}
    >
      {children}
    </Page>
  );
}

function MemberRow({
  org,
  member,
  groups,
  onChanged,
}: {
  org: Org;
  member: OrgMember;
  groups: string[];
  onChanged: (text: string, undo?: () => Promise<void>) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [asking, setAsking] = useState<"owner" | "remove">();
  const name = nameOf(member);
  const canManage = !member.you && manages(org.role, member.role);

  const put = async (role: OrgRole) => {
    await (
      await client("change a role")
    ).setOrgMemberRole(org.id, member.id, { role });
  };
  const setRole = (role: OrgRole) => {
    const before = member.role;
    void action.run(
      async () => {
        await put(role);
        setAsking(undefined);
        onChanged(`${name} is now ${ROLE_WORDS[role]}`, async () => {
          await put(before);
          onChanged(`${name} is ${ROLE_WORDS[before]} again`);
        });
      },
      (error) => `${problemText(error)} Their role is unchanged.`,
    );
  };
  const close = () => {
    action.clear();
    setAsking(undefined);
  };

  return (
    <Row
      lead={<MemberAvatar member={member} />}
      hot={{
        picture: { svg: memberSvg(member.avatarSeed) },
      }}
      name={
        <>
          {name}
          {member.you ? <Chip tone="you">You</Chip> : null}
        </>
      }
      detail={[
        ...groups,
        `${groups.length > 0 ? "joined" : "Joined"} ${formatDay(member.joinedAt, "short")}`,
      ].join(" · ")}
      end={
        canManage ? (
          <Popover
            label={`${name}'s role`}
            disabled={action.busy}
            trigger={
              <>
                {ROLE_WORDS[member.role]}
                <IconExpandAll size={12} />
              </>
            }
            options={[
              ...ROLES.filter((each) => manages(org.role, each)).map(
                (each) => ({
                  key: each,
                  name: ROLE_WORDS[each],
                  detail: ROLE_DETAIL[each],
                  chosen: each === member.role,
                  choose: () => {
                    action.clear();
                    if (each === "owner" && member.role !== "owner")
                      setAsking("owner");
                    else if (each !== member.role) setRole(each);
                  },
                }),
              ),
              {
                key: "remove",
                name: "Remove from org",
                bad: true,
                choose: () => {
                  action.clear();
                  setAsking("remove");
                },
              },
            ]}
          />
        ) : (
          ROLE_WORDS[member.role]
        )
      }
    >
      {asking === "owner" ? (
        <Confirm
          busy={action.busy}
          problem={action.problem}
          label="Make owner"
          onCancel={close}
          onConfirm={() => setRole("owner")}
        >
          <p>
            {name} gets everything you have in {org.name}: its people, its bill
            and deleting it.
          </p>
        </Confirm>
      ) : asking === "remove" ? (
        <Confirm
          danger
          busy={action.busy}
          problem={action.problem}
          label={`Remove ${name}`}
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("remove someone")
                ).removeOrgMember(org.id, member.id);
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
      ) : action.problem ? (
        <Problem>{action.problem}</Problem>
      ) : null}
    </Row>
  );
}

function Invites({
  org,
  invites,
  made,
  onChanged,
}: {
  org: Org;
  invites: OrgInvite[];
  made?: OrgInviteCreated;
  onChanged: (text: string) => void;
}) {
  return (
    <Card
      label="Invite links"
      count={invites.length || undefined}
      foot={
        invites.length > 0
          ? "You send each link yourself. It works once and holds a seat for 7 days."
          : undefined
      }
    >
      {invites.length > 0 ? (
        <Rows>
          {invites.map((invite) => (
            <InviteRow
              key={invite.id}
              org={org}
              invite={invite}
              link={made?.invite.id === invite.id ? made.link : undefined}
              onChanged={onChanged}
            />
          ))}
        </Rows>
      ) : (
        <Empty>
          No open links. You make a link and send it yourself; we mail nobody.
        </Empty>
      )}
    </Card>
  );
}

function InviteRow({
  org,
  invite,
  link,
  onChanged,
}: {
  org: Org;
  invite: OrgInvite;
  link?: string;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [copied, setCopied] = useState(false);
  return (
    <Row
      lead={
        <Tile>
          <IconLink size={14} />
        </Tile>
      }
      name={
        link ? (
          <span className="mono">{link.replace(/^https?:\/\//, "")}</span>
        ) : (
          `Made ${formatDay(invite.createdAt, "short")}${invite.invitedBy ? ` by ${invite.invitedBy}` : ""}`
        )
      }
      detail={
        link
          ? `Joins as ${ROLE_WORDS[invite.role]} · shown once · ends in 7 days`
          : `Joins as ${ROLE_WORDS[invite.role]} · ends ${formatDay(invite.expiresAt, "short")}`
      }
      end={
        <>
          {link ? (
            <Btn
              small
              quiet
              onClick={() => void copyText(link).then(setCopied)}
            >
              <IconCopy size={13} />
              {copied ? "Copied" : "Copy"}
            </Btn>
          ) : null}
          {manages(org.role, invite.role) ? (
            <Txt
              bad
              disabled={action.busy}
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
              {action.busy ? "Revoking…" : "Revoke"}
            </Txt>
          ) : null}
        </>
      }
    >
      {action.problem ? <Problem>{action.problem}</Problem> : null}
    </Row>
  );
}

function Groups({
  org,
  members,
  teams,
  editable,
  onChanged,
}: {
  org: Org;
  members: OrgMember[];
  teams: OrgTeam[];
  editable: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [name, setName] = useState<string>();
  if (teams.length === 0 && !editable) return null;

  const close = () => {
    action.clear();
    setName(undefined);
  };
  const make = () => {
    const wanted = name?.trim();
    if (!wanted) return;
    void action.run(
      async () => {
        await (
          await client("make a group")
        ).createOrgTeam(org.id, { name: wanted });
        setName(undefined);
        onChanged(`Made ${wanted}`);
      },
      (error) => `${problemText(error)} No group was made.`,
    );
  };

  return (
    <Card
      label="Groups"
      count={teams.length || undefined}
      aside={
        editable && name === undefined ? (
          <Txt onClick={() => setName("")}>
            <IconPlus size={13} />
            New group
          </Txt>
        ) : null
      }
    >
      {name !== undefined ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            make();
          }}
        >
          <Confirm
            busy={action.busy}
            problem={action.problem}
            label="Make group"
            unchanged={!name.trim()}
            onCancel={close}
            onConfirm={make}
          >
            <Field
              aria-label="Group name"
              placeholder="Design"
              value={name}
              maxLength={64}
              autoFocus
              onChange={(event) => setName(event.target.value)}
            />
          </Confirm>
        </form>
      ) : null}
      {teams.length > 0 ? (
        <Rows>
          {teams.map((each) => (
            <GroupRow
              key={each.id}
              org={org}
              group={each}
              members={members}
              editable={editable}
              onChanged={onChanged}
            />
          ))}
        </Rows>
      ) : name === undefined ? (
        <Empty>No groups. One is a short list of people you name.</Empty>
      ) : null}
    </Card>
  );
}

function GroupRow({
  org,
  group,
  members,
  editable,
  onChanged,
}: {
  org: Org;
  group: OrgTeam;
  members: OrgMember[];
  editable: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [open, setOpen] = useState<"people" | "rename" | "delete">();
  const [name, setName] = useState(group.name);
  const inside = members.filter((each) => group.members.includes(each.id));
  const close = () => {
    action.clear();
    setOpen(undefined);
  };
  const more = inside.length - 3;

  return (
    <Row
      lead={
        <Tile>
          <IconStack size={14} />
        </Tile>
      }
      name={group.name}
      detail={
        inside.length === 0
          ? "Nobody yet"
          : `${inside.slice(0, 3).map(nameOf).join(", ")}${more > 0 ? ` and ${more} more` : ""}`
      }
      end={
        <>
          <span className="stack" aria-hidden="true">
            {inside.slice(0, 3).map((each) => (
              <MemberAvatar key={each.id} member={each} />
            ))}
          </span>
          {editable ? (
            <Popover
              label={`Change ${group.name}`}
              trigger={<IconMore size={14} />}
              options={[
                {
                  key: "people",
                  name: "People",
                  choose: () => setOpen("people"),
                },
                {
                  key: "rename",
                  name: "Rename",
                  choose: () => {
                    setName(group.name);
                    setOpen("rename");
                  },
                },
                {
                  key: "delete",
                  name: "Delete group",
                  bad: true,
                  choose: () => setOpen("delete"),
                },
              ]}
            />
          ) : null}
        </>
      }
    >
      {!editable ? null : open === "people" ? (
        <div className="box">
          {members.map((member) => (
            <GroupPick
              key={member.id}
              org={org}
              group={group}
              member={member}
              onChanged={onChanged}
            />
          ))}
          <div className="do">
            <Btn small quiet onClick={close}>
              Done
            </Btn>
          </div>
        </div>
      ) : open === "rename" ? (
        <Confirm
          busy={action.busy}
          problem={action.problem}
          label="Rename"
          unchanged={!name.trim() || name.trim() === group.name}
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                const renamed = await (
                  await client("rename a group")
                ).renameOrgTeam(org.id, group.id, { name: name.trim() });
                setOpen(undefined);
                onChanged(`${group.name} is now ${renamed.name}`);
              },
              (error) => `${problemText(error)} Its name is unchanged.`,
            )
          }
        >
          <Field
            aria-label="Group name"
            value={name}
            maxLength={64}
            onChange={(event) => setName(event.target.value)}
          />
        </Confirm>
      ) : open === "delete" ? (
        <Confirm
          danger
          busy={action.busy}
          problem={action.problem}
          label={`Delete ${group.name}`}
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("delete a group")
                ).deleteOrgTeam(org.id, group.id);
                onChanged(`Deleted ${group.name}`);
              },
              (error) => `${problemText(error)} The group is still there.`,
            )
          }
        >
          <p>Its members stay in {org.name}; only the list goes.</p>
        </Confirm>
      ) : null}
    </Row>
  );
}

function GroupPick({
  org,
  group,
  member,
  onChanged,
}: {
  org: Org;
  group: OrgTeam;
  member: OrgMember;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const name = nameOf(member);
  const inside = group.members.includes(member.id);
  return (
    <>
      <div className="line">
        <MemberAvatar member={member} size={20} />
        <span>{name}</span>
        <Switch
          label={`${name} in ${group.name}`}
          on={inside}
          disabled={action.busy}
          onChange={(on) =>
            void action.run(
              async () => {
                const routes = await client("change a group");
                if (on)
                  await routes.addOrgTeamMember(org.id, group.id, member.id);
                else
                  await routes.removeOrgTeamMember(org.id, group.id, member.id);
                onChanged(
                  on
                    ? `${name} is in ${group.name}`
                    : `${name} is out of ${group.name}`,
                );
              },
              (error) => `${problemText(error)} The group is unchanged.`,
            )
          }
        />
      </div>
      <Problem>{action.problem}</Problem>
    </>
  );
}

function Leave({
  org,
  members,
  onLeft,
}: {
  org: Org;
  members: OrgMember[];
  onLeft: () => Promise<void>;
}) {
  const client = useClient();
  const say = useToast();
  const action = useAction();
  const [asking, setAsking] = useState(false);
  const soleOwner =
    org.role === "owner" &&
    members.filter((each) => each.role === "owner").length === 1;
  return (
    <Card label="Leave">
      <Rows>
        <Row
          lead={
            <Tile>
              <IconLogOut size={14} />
            </Tile>
          }
          name={`Leave ${org.name}`}
          detail={
            soleOwner
              ? "Make someone else an owner first."
              : "Your hosts and account stay yours."
          }
          end={
            soleOwner || asking ? null : (
              <Txt bad onClick={() => setAsking(true)}>
                Leave
              </Txt>
            )
          }
        >
          {asking && !soleOwner ? (
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
                    say(`You left ${org.name}`);
                    await onLeft();
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
          ) : null}
        </Row>
      </Rows>
    </Card>
  );
}
