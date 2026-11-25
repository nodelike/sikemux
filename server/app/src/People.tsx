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
  IconCheck,
  IconChevron,
  IconCopy,
  IconHourglass,
  IconLink,
  IconMore,
  IconPause,
  IconPlus,
  IconTag,
  IconTrash,
  IconUndo,
  IconUsers,
} from "@shared/ui/Icons";
import { useEffect, useState, type MouseEvent } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./client.ts";
import { useCheer } from "./Field.tsx";
import { useOrgMark } from "./logos.ts";
import "./members.css";
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
import {
  copyText,
  freeSeats,
  MemberAvatar,
  nameOf,
  people,
  SeatsStrip,
} from "./Seats.tsx";
import {
  Btn,
  btnClass,
  Chip,
  Confirm,
  Dialog,
  Ending,
  Nothing,
  Notice,
  Page,
  Popover,
  Problem,
  Seal,
  Search,
  Seg,
  Switch,
  Table,
  Td,
  Tr,
  useAction,
  useToast,
  Who,
} from "./ui.tsx";

const ROLES: OrgRole[] = ["owner", "admin", "user"];

const ROLE_DETAIL: Record<OrgRole, string> = {
  owner: "Everything, with the bill",
  admin: "People, domains and rules",
  user: "Works in the org",
};

const ROLE_FILTER_LABELS: Record<"any" | OrgRole, string> = {
  any: "Any role",
  owner: "Owners",
  admin: "Admins",
  user: "Members",
};

const MEMBER_COLS =
  "minmax(210px, 1.6fr) 112px minmax(140px, 1.1fr) 118px 44px";

const roleArticle = (role: OrgRole) =>
  role === "owner" ? "an Owner" : role === "admin" ? "an Admin" : "a Member";

function expiresIn(iso: string): string {
  const days = Math.ceil((Date.parse(iso) - Date.now()) / 86_400_000);
  if (!Number.isFinite(days)) return `Ends ${formatDay(iso, "short")}`;
  if (days <= 0) return "Ends today";
  if (days === 1) return "Ends tomorrow";
  return `${days} days left`;
}

function ownerNames(members: OrgMember[]): string {
  const names = members
    .filter((each) => each.role === "owner")
    .map((each) => each.name)
    .filter((each): each is string => Boolean(each));
  if (names.length === 0) return "an owner";
  if (names.length === 1) return names[0];
  return `${names.slice(0, -1).join(", ")} or ${names[names.length - 1]}`;
}

function usePhone(): boolean {
  const [phone, setPhone] = useState(
    () =>
      typeof matchMedia !== "undefined" &&
      matchMedia("(max-width: 860px)").matches,
  );
  useEffect(() => {
    const query = matchMedia("(max-width: 860px)");
    const update = () => setPhone(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return phone;
}

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
  const phone = usePhone();
  const [query, setQuery] = useState("");
  const [roleFilter, setRoleFilter] = useState<"any" | OrgRole>("any");
  const [only, setOnly] = useState<"all" | "pending">("all");
  const [inviting, setInviting] = useState(false);
  const [made, setMade] = useState<OrgInviteCreated>();
  const [recentLinks, setRecentLinks] = useState<Record<string, string>>({});
  const [inlineProblem, setInlineProblem] = useState<string>();
  const [groupsFor, setGroupsFor] = useState<string>();
  const [askingLeave, setAskingLeave] = useState(false);

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
    setInlineProblem(undefined);
    say(text, undo);
    loaded.again();
    void reload();
  };

  const scene = orgScene(org.id);
  const mark = useOrgMark(org);
  const { members, invites, teams } = loaded.data ?? {};
  const free = freeSeats(org);
  const open = changeable(org, team);
  const ended = renewable(org, team);
  const billing = orgPath(org.id, "billing");
  const canManageOrg = runs(org) && open;

  if (loaded.problem)
    return (
      <Page
        title="Members"
        lede="Everyone in the org."
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Nothing
          title="Members didn't load"
          actions={<Btn onClick={loaded.again}>Try again</Btn>}
        >
          {loaded.problem} Nothing in the org has changed.
        </Nothing>
      </Page>
    );

  const boss = members?.find((each) => each.role === "owner");
  const bossName = boss?.name ?? "An owner";
  const soleOwner = Boolean(
    members &&
    org.role === "owner" &&
    members.filter((each) => each.role === "owner").length === 1,
  );

  const notice = frozen(org) ? (
    ended ? (
      <Notice
        icon={<IconHourglass size={17} />}
        tone="cold"
        title={`${org.name} is frozen`}
        actions={
          org.role === "owner" ? (
            <a
              className={btnClass({})}
              href={billing}
              onClick={follow(billing)}
            >
              Renew Team
            </a>
          ) : (
            <span className="m-dim">{bossName} can renew it</span>
          )
        }
      >
        Its Team plan ended. Nothing is deleted.
      </Notice>
    ) : (
      <Notice icon={<IconPause size={17} />} title={`${org.name} is paused`}>
        Team isn't open to your account now. Nothing is deleted.
      </Notice>
    )
  ) : !members ? null : !open ? (
    <Notice
      icon={<IconPause size={17} />}
      title="Team isn't open to your account now"
    >
      You can still read, hand over and leave.
    </Notice>
  ) : !runs(org) ? (
    <Notice icon={<IconUsers size={17} />} title="Want to bring someone in?">
      Ask {ownerNames(members)} for an invite.
    </Notice>
  ) : free === 0 ? (
    org.preview ? (
      <Notice
        icon={<IconUsers size={17} />}
        title={`The preview holds ${org.seats.limit}`}
      >
        Take back an invite or remove someone to free a seat.
      </Notice>
    ) : (
      <Notice icon={<IconUsers size={17} />} title="Every seat is taken">
        {org.role === "owner"
          ? "Add seats, or take back an invite to free one."
          : `${bossName} can add seats.`}
      </Notice>
    )
  ) : null;

  const trimmed = query.trim().toLowerCase();
  const teamsOf = (memberId: string) =>
    teams?.filter((each) => each.members.includes(memberId)) ?? [];

  const filteredInvites = (invites ?? []).filter((invite) => {
    if (roleFilter !== "any" && invite.role !== roleFilter) return false;
    if (!trimmed) return true;
    const text =
      `invite link ${ROLE_WORDS[invite.role]} ${invite.invitedBy ?? ""}`.toLowerCase();
    return text.includes(trimmed);
  });

  const filteredMembers =
    only === "pending"
      ? []
      : (members ?? []).filter((member) => {
          if (roleFilter !== "any" && member.role !== roleFilter) return false;
          if (!trimmed) return true;
          const groupNames = teamsOf(member.id)
            .map((each) => each.name)
            .join(" ");
          return `${nameOf(member)} ${groupNames}`
            .toLowerCase()
            .includes(trimmed);
        });

  const hasFilter = trimmed !== "" || roleFilter !== "any" || only !== "all";
  const clearFilters = () => {
    setQuery("");
    setRoleFilter("any");
    setOnly("all");
  };

  const openInvite = () => {
    setMade(undefined);
    setInviting(true);
  };

  const inviteButton = !canManageOrg ? null : free === 0 &&
    !org.preview &&
    org.role === "owner" ? (
    <a className={btnClass({})} href={billing} onClick={follow(billing)}>
      <IconPlus size={14} />
      Add seats
    </a>
  ) : free === 0 ? (
    <Btn disabled>
      <IconPlus size={14} />
      Invite
    </Btn>
  ) : (
    <Btn onClick={openInvite}>
      <IconPlus size={14} />
      Invite
    </Btn>
  );

  const tools = (
    <>
      <Search
        label="Find a person or group"
        value={query}
        onChange={setQuery}
      />
      <span className="wide">
        <Popover
          look="pick"
          align="left"
          label="Filter by role"
          trigger={
            <>
              <span>{ROLE_FILTER_LABELS[roleFilter]}</span>
              <IconChevron size={12} className="m-down" />
            </>
          }
          options={(["any", "owner", "admin", "user"] as const).map((key) => ({
            key,
            name: ROLE_FILTER_LABELS[key],
            chosen: roleFilter === key,
            choose: () => setRoleFilter(key),
          }))}
        />
      </span>
      {runs(org) ? (
        <Seg
          label="Show"
          value={only}
          onChange={setOnly}
          options={[
            { value: "all", name: "Everyone" },
            {
              value: "pending",
              name: invites?.length ? `Invited ${invites.length}` : "Invited",
            },
          ]}
        />
      ) : null}
      <span className="gap" />
      {members ? (
        <SeatsStrip
          org={org}
          href={org.role === "owner" ? billing : undefined}
          onClick={org.role === "owner" ? follow(billing) : undefined}
        />
      ) : null}
      {inviteButton}
    </>
  );

  const lede = !members
    ? "Everyone in the org."
    : members.length === 1 && !invites?.length
      ? "Only you so far."
      : `${people(members.length)}${invites?.length ? `, ${invites.length} invited` : ""}.`;

  const groupMember = groupsFor
    ? members?.find((each) => each.id === groupsFor)
    : undefined;

  return (
    <Page
      title="Members"
      lede={lede}
      notice={notice}
      tools={tools}
      mark={mark}
      scene={scene}
      tone={frozen(org) ? "waiting" : undefined}
    >
      {inlineProblem ? <Problem>{inlineProblem}</Problem> : null}

      {!members || !invites || !teams ? (
        <TableSkeleton />
      ) : filteredMembers.length === 0 && filteredInvites.length === 0 ? (
        <Nothing
          title={
            only === "pending" && !trimmed && roleFilter === "any"
              ? "No invites are waiting"
              : "Nobody matches"
          }
          actions={
            <Btn quiet onClick={clearFilters}>
              Clear
            </Btn>
          }
        >
          {only === "pending" && !trimmed && roleFilter === "any"
            ? "Make a link with Invite when you want to bring someone in."
            : "Try another name, group or role."}
        </Nothing>
      ) : (
        <div>
          <Table
            label="Members"
            cols={MEMBER_COLS}
            heads={["Member", "Role", "Groups", "Joined", ""]}
          >
            {filteredInvites.map((invite) => (
              <InviteRow
                key={invite.id}
                org={org}
                invite={invite}
                link={recentLinks[invite.id]}
                fresh={made?.invite.id === invite.id}
                onChanged={changed}
              />
            ))}
            {filteredMembers.map((member) => (
              <MemberRow
                key={member.id}
                org={org}
                member={member}
                groups={teamsOf(member.id)}
                open={open}
                canEditGroups={canManageOrg && teams.length > 0}
                phone={phone}
                onOpenGroups={() => setGroupsFor(member.id)}
                onChanged={changed}
                onProblem={setInlineProblem}
              />
            ))}
          </Table>
          {members.length === 1 &&
          invites.length === 0 &&
          !hasFilter &&
          canManageOrg &&
          free > 0 ? (
            <div className="m-solo">
              <span className="m-ghost" aria-hidden="true">
                <IconPlus size={13} />
              </span>
              <div>
                <b>It's only you here</b>
                <p>
                  Invite the people you build with. {free} of your{" "}
                  {org.seats.limit} seats are free.
                </p>
              </div>
              <Btn quiet onClick={openInvite}>
                <IconPlus size={14} />
                Invite
              </Btn>
            </div>
          ) : null}
        </div>
      )}

      {members ? (
        <Ending
          title={`Leave ${org.name}`}
          actions={
            <Btn
              bad={!soleOwner}
              quiet={soleOwner}
              disabled={soleOwner}
              onClick={() => setAskingLeave(true)}
            >
              Leave
            </Btn>
          }
        >
          {soleOwner
            ? "Make someone else an owner first."
            : "Your hosts and account stay yours. You give up your seat and the Pro it gives you."}
        </Ending>
      ) : null}

      {inviting && canManageOrg ? (
        <InviteDialog
          org={org}
          free={free}
          made={made}
          onMade={(created) => {
            setMade(created);
            setRecentLinks((prev) => ({
              ...prev,
              [created.invite.id]: created.link,
            }));
            loaded.again();
            void reload();
          }}
          onAnother={() => setMade(undefined)}
          onClose={() => setInviting(false)}
        />
      ) : null}

      {groupMember && teams && canManageOrg ? (
        <MemberGroupsDialog
          org={org}
          member={groupMember}
          teams={teams}
          onChanged={changed}
          onClose={() => setGroupsFor(undefined)}
        />
      ) : null}

      {askingLeave && !soleOwner ? (
        <LeaveConfirm
          org={org}
          onCancel={() => setAskingLeave(false)}
          onLeft={onLeft}
        />
      ) : null}
    </Page>
  );
}

function TableSkeleton() {
  return (
    <Table
      label="Members"
      cols={MEMBER_COLS}
      heads={["Member", "Role", "Groups", "Joined", ""]}
    >
      {Array.from({ length: 7 }, (_, at) => (
        <Tr key={at}>
          <Td>
            <span className="who">
              <span className="skel m-skel-av" />
              <span
                className="skel m-skel-bar"
                style={{ width: 110 + ((at * 37) % 60) }}
              />
            </span>
          </Td>
          <Td wide>
            <span className="skel m-skel-bar" style={{ width: 52 }} />
          </Td>
          <Td wide>
            <span
              className="skel m-skel-chip"
              style={{ width: 70 + ((at * 23) % 50) }}
            />
          </Td>
          <Td wide>
            <span className="skel m-skel-bar" style={{ width: 70 }} />
          </Td>
          <Td end />
        </Tr>
      ))}
    </Table>
  );
}

function MemberRow({
  org,
  member,
  groups,
  open,
  canEditGroups,
  phone,
  onOpenGroups,
  onChanged,
  onProblem,
}: {
  org: Org;
  member: OrgMember;
  groups: OrgTeam[];
  open: boolean;
  canEditGroups: boolean;
  phone: boolean;
  onOpenGroups: () => void;
  onChanged: (text: string, undo?: () => Promise<void>) => void;
  onProblem: (text?: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [asking, setAsking] = useState<"owner" | "remove">();
  const name = nameOf(member);
  const canManage = !member.you && open && manages(org.role, member.role);
  const manageableRoles = ROLES.filter((each) => manages(org.role, each));

  const putRole = async (role: OrgRole) => {
    await (
      await client("change a role")
    ).setOrgMemberRole(org.id, member.id, { role });
  };

  const setRole = (role: OrgRole, inDialog = false) => {
    const before = member.role;
    onProblem(undefined);
    void action.run(
      async () => {
        await putRole(role);
        setAsking(undefined);
        onChanged(`${name} is now ${ROLE_WORDS[role]}`, async () => {
          await putRole(before);
          onChanged(`${name} is ${ROLE_WORDS[before]} again`);
        });
      },
      (error) => {
        const words = `${problemText(error)} Their role is unchanged.`;
        if (!inDialog) onProblem(words);
        return words;
      },
    );
  };

  const chooseRole = (role: OrgRole) => {
    action.clear();
    if (role === "owner" && member.role !== "owner") setAsking("owner");
    else if (role !== member.role) setRole(role);
  };

  const close = () => {
    action.clear();
    setAsking(undefined);
  };

  const menuOptions = !canManage
    ? []
    : [
        ...(phone
          ? manageableRoles.map((each) => ({
              key: `role-${each}`,
              name: ROLE_WORDS[each],
              detail: ROLE_DETAIL[each],
              chosen: each === member.role,
              choose: () => chooseRole(each),
            }))
          : []),
        ...(canEditGroups
          ? [
              {
                key: "groups",
                name: "Change groups",
                icon: <IconTag size={14} />,
                choose: onOpenGroups,
              },
            ]
          : []),
        {
          key: "remove",
          name: `Remove from ${org.name}`,
          icon: <IconTrash size={14} />,
          bad: true,
          choose: () => {
            action.clear();
            setAsking("remove");
          },
        },
      ];

  return (
    <Tr>
      <Td>
        <Who
          lead={<MemberAvatar member={member} size={28} />}
          name={
            <>
              {name}
              {member.you ? <Chip tone="you">You</Chip> : null}
            </>
          }
          phone={[
            ROLE_WORDS[member.role],
            ...groups.map((each) => each.name),
          ].join(", ")}
        />
      </Td>
      <Td wide>
        {canManage ? (
          <Popover
            align="left"
            label={`${name}'s role`}
            disabled={action.busy}
            trigger={
              <>
                {ROLE_WORDS[member.role]}
                <IconChevron size={12} className="m-down" />
              </>
            }
            options={manageableRoles.map((each) => ({
              key: each,
              name: ROLE_WORDS[each],
              detail: ROLE_DETAIL[each],
              chosen: each === member.role,
              choose: () => chooseRole(each),
            }))}
          />
        ) : (
          <span className="m-plainrole">{ROLE_WORDS[member.role]}</span>
        )}
      </Td>
      <Td wide>
        {groups.length > 0 ? (
          <span className="chips">
            {groups.slice(0, 2).map((each) => (
              <Chip key={each.id}>{each.name}</Chip>
            ))}
            {groups.length > 2 ? <Chip>+{groups.length - 2}</Chip> : null}
          </span>
        ) : (
          <span className="m-none">None</span>
        )}
      </Td>
      <Td wide>{formatDay(member.joinedAt, "short")}</Td>
      <Td end>
        {menuOptions.length > 0 ? (
          <Popover
            look="icon"
            label={`More for ${name}`}
            disabled={action.busy}
            trigger={<IconMore size={15} />}
            options={menuOptions}
          />
        ) : null}
      </Td>
      {asking === "owner" ? (
        <Confirm
          title={`Make ${name} an owner?`}
          busy={action.busy}
          problem={action.problem}
          label="Make owner"
          onCancel={close}
          onConfirm={() => setRole("owner", true)}
        >
          <p>
            {name} gets everything you have in {org.name}: its people, its bill
            and deleting it.
          </p>
        </Confirm>
      ) : asking === "remove" ? (
        <Confirm
          danger
          title={`Remove ${name}?`}
          busy={action.busy}
          problem={action.problem}
          label="Remove"
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("remove someone")
                ).removeOrgMember(org.id, member.id);
                setAsking(undefined);
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
    </Tr>
  );
}

function InviteRow({
  org,
  invite,
  link,
  fresh,
  onChanged,
}: {
  org: Org;
  invite: OrgInvite;
  link?: string;
  fresh?: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const say = useToast();
  const action = useAction();
  const [revoking, setRevoking] = useState(false);
  const canRevoke = manages(org.role, invite.role);
  const expiry = expiresIn(invite.expiresAt);

  const options = [
    ...(link
      ? [
          {
            key: "copy",
            name: "Copy link",
            icon: <IconCopy size={14} />,
            choose: () =>
              void copyText(link).then((ok) =>
                say(ok ? "Link copied" : "Couldn't copy link"),
              ),
          },
        ]
      : []),
    ...(canRevoke
      ? [
          {
            key: "revoke",
            name: "Take back the invite",
            detail: "Frees the seat",
            icon: <IconUndo size={14} />,
            bad: true,
            choose: () => {
              action.clear();
              setRevoking(true);
            },
          },
        ]
      : []),
  ];

  const close = () => {
    action.clear();
    setRevoking(false);
  };

  return (
    <Tr soft lit={fresh}>
      <Td>
        <Who
          lead={
            <span className="m-ghost" aria-hidden="true">
              <IconLink size={13} />
            </span>
          }
          name={
            <>
              <span>Invite link</span>
              <Seal tone="wait">Invited</Seal>
            </>
          }
          detail={`${invite.invitedBy ? `By ${invite.invitedBy} · ` : ""}${expiry}`}
          phone={ROLE_WORDS[invite.role]}
        />
      </Td>
      <Td wide>
        <span className="m-plainrole">{ROLE_WORDS[invite.role]}</span>
      </Td>
      <Td wide>
        <span className="m-none">—</span>
      </Td>
      <Td wide>
        <span className="m-none">—</span>
      </Td>
      <Td end>
        {options.length > 0 ? (
          <Popover
            look="icon"
            label="More for invite link"
            disabled={action.busy}
            trigger={<IconMore size={15} />}
            options={options}
          />
        ) : null}
      </Td>
      {revoking ? (
        <Confirm
          danger
          title="Take back this invite?"
          busy={action.busy}
          problem={action.problem}
          label="Take back"
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("revoke an invite")
                ).revokeOrgInvite(org.id, invite.id);
                setRevoking(false);
                onChanged("Invite link revoked. Its seat is free.");
              },
              (error) => `${problemText(error)} The link still works.`,
            )
          }
        >
          <p>The link stops working and the seat is free again.</p>
        </Confirm>
      ) : null}
    </Tr>
  );
}

function InviteDialog({
  org,
  free,
  made,
  onMade,
  onAnother,
  onClose,
}: {
  org: Org;
  free: number;
  made?: OrgInviteCreated;
  onMade: (created: OrgInviteCreated) => void;
  onAnother: () => void;
  onClose: () => void;
}) {
  const client = useClient();
  const cheer = useCheer();
  const action = useAction();
  const roles = ROLES.filter((each) => manages(org.role, each)).reverse();
  const [role, setRole] = useState<OrgRole>("user");
  const [copied, setCopied] = useState(false);

  if (made)
    return (
      <Dialog
        wide
        title="Link ready"
        onClose={onClose}
        foot={
          <>
            <span className="gap" />
            {free > 0 ? (
              <Btn
                quiet
                onClick={() => {
                  setCopied(false);
                  onAnother();
                }}
              >
                Make another
              </Btn>
            ) : null}
            <Btn onClick={onClose}>Done</Btn>
          </>
        }
      >
        <div className="m-sentrow">
          <span className="m-tick" aria-hidden="true">
            <IconLink size={13} />
          </span>
          <span className="m-sx">
            <b>{ROLE_WORDS[made.invite.role]} invite</b>
            <code>{made.link.replace(/^https?:\/\//, "")}</code>
          </span>
          <Btn
            small
            quiet
            className={copied ? "m-did" : undefined}
            onClick={() =>
              void copyText(made.link).then((ok) => {
                if (!ok) return;
                setCopied(true);
                setTimeout(() => setCopied(false), 1600);
              })
            }
          >
            {copied ? <IconCheck size={13} /> : <IconCopy size={13} />}
            {copied ? "Copied" : "Copy"}
          </Btn>
        </div>
        <p className="m-hint">
          Copy it now: it is shown only here and works once, until{" "}
          {formatDay(made.invite.expiresAt, "long")}.
        </p>
      </Dialog>
    );

  const submit = () => {
    void action.run(
      async () => {
        const created = await (
          await client("make an invite link")
        ).createOrgInvite(org.id, { role });
        cheer();
        onMade(created);
      },
      (error) => `${problemText(error)} No link was made.`,
    );
  };

  return (
    <Dialog
      wide
      title={`Invite to ${org.name}`}
      busy={action.busy}
      onClose={onClose}
      onSubmit={submit}
      foot={
        <>
          <span className="m-dim">
            {free} free {free === 1 ? "seat" : "seats"}
          </span>
          <span className="gap" />
          <Btn quiet disabled={action.busy} onClick={onClose}>
            Cancel
          </Btn>
          <Btn type="submit" busy={action.busy}>
            Make a link
          </Btn>
        </>
      }
    >
      <div className="m-fl">
        <label>Role</label>
        <Seg
          label="Role"
          value={role}
          disabled={action.busy}
          onChange={setRole}
          options={roles.map((each) => ({
            value: each,
            name: ROLE_WORDS[each],
          }))}
        />
        <p className="m-hint">{ROLE_DETAIL[role]}.</p>
      </div>
      <Problem>{action.problem}</Problem>
      <div className="m-what">
        <span className="cap">What happens next</span>
        <ol className="m-nexts">
          <li>You send them the link yourself. Sikemux mails nobody.</li>
          <li>They sign in, or make an account, and see who asked them.</li>
          <li>
            They join as {roleArticle(role)}. Unused links work once and hold a
            seat for 7 days.
          </li>
        </ol>
      </div>
    </Dialog>
  );
}

function MemberGroupsDialog({
  org,
  member,
  teams,
  onChanged,
  onClose,
}: {
  org: Org;
  member: OrgMember;
  teams: OrgTeam[];
  onChanged: (text: string) => void;
  onClose: () => void;
}) {
  const client = useClient();
  const [query, setQuery] = useState("");
  const [busyId, setBusyId] = useState<string>();
  const [problem, setProblem] = useState<string>();
  const name = nameOf(member);
  const trimmed = query.trim().toLowerCase();
  const shown = trimmed
    ? teams.filter((each) => each.name.toLowerCase().includes(trimmed))
    : teams;

  const toggle = async (team: OrgTeam, on: boolean) => {
    setBusyId(team.id);
    setProblem(undefined);
    try {
      const routes = await client("change a group");
      if (on) await routes.addOrgTeamMember(org.id, team.id, member.id);
      else await routes.removeOrgTeamMember(org.id, team.id, member.id);
      onChanged(
        on ? `${name} is in ${team.name}` : `${name} is out of ${team.name}`,
      );
    } catch (error) {
      setProblem(`${problemText(error)} The group is unchanged.`);
    } finally {
      setBusyId(undefined);
    }
  };

  return (
    <Dialog
      title={`${name}'s groups`}
      busy={busyId !== undefined}
      onClose={onClose}
      foot={<Btn onClick={onClose}>Done</Btn>}
    >
      {teams.length > 0 ? (
        <div className="m-dlg-search">
          <Search label="Find a group" value={query} onChange={setQuery} />
        </div>
      ) : null}
      <Problem>{problem}</Problem>
      {teams.length === 0 ? (
        <p className="m-hint">No groups yet. Make one on the Groups page.</p>
      ) : shown.length === 0 ? (
        <p className="m-hint">No group matches "{query}".</p>
      ) : (
        <div className="m-picklist">
          {shown.map((team) => {
            const inside = team.members.includes(member.id);
            return (
              <div key={team.id} className="m-pickrow">
                <span className="m-pickrow-who">
                  <b>{team.name}</b>
                  <small>{people(team.members.length)}</small>
                </span>
                <Switch
                  label={`${name} in ${team.name}`}
                  on={inside}
                  disabled={busyId === team.id}
                  onChange={(on) => void toggle(team, on)}
                />
              </div>
            );
          })}
        </div>
      )}
    </Dialog>
  );
}

function LeaveConfirm({
  org,
  onCancel,
  onLeft,
}: {
  org: Org;
  onCancel: () => void;
  onLeft: () => Promise<void>;
}) {
  const client = useClient();
  const say = useToast();
  const action = useAction();
  return (
    <Confirm
      danger
      title={`Leave ${org.name}?`}
      busy={action.busy}
      problem={action.problem}
      label={`Leave ${org.name}`}
      onCancel={onCancel}
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
  );
}
