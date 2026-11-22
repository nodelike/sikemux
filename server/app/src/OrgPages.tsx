import type {
  Org,
  OrgAuditEntry,
  OrgDomain,
  OrgDomainJoining,
  OrgMember,
  OrgOffer,
  OrgPolicy,
  OrgRole,
  OrgRuleAdding,
  OrgTeam,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconActivity,
  IconCircleAlert,
  IconCopy,
  IconGlobe,
  IconLock,
  IconPlus,
  IconShield,
  IconStack,
  IconUsers,
} from "@shared/ui/Icons";
import {
  useCallback,
  useEffect,
  useEffectEvent,
  useState,
  type ReactNode,
} from "react";

import { problemText } from "./api.ts";
import { PersonAvatar } from "./Avatar.tsx";
import { useClient } from "./Billing.tsx";
import {
  Confirm,
  ItemRow,
  PageHead,
  Rows,
  Section,
  Segmented,
  Switch,
  useAction,
  useNotice,
} from "./ui.tsx";

const CHOSEN_KEY = "sikemux.org";

export const ROLE_WORDS: Record<OrgRole, string> = {
  owner: "Owner",
  admin: "Admin",
  user: "User",
};

const runs = (org: Org) => org.role === "owner" || org.role === "admin";

/** The orgs the person is in, and the one the org pages are about, kept across them for the tab. */
export function useOrgs() {
  const client = useClient();
  const [orgs, setOrgs] = useState<Org[]>();
  const [problem, setProblem] = useState<string>();
  const [chosen, setChosen] = useState(
    () => sessionStorage.getItem(CHOSEN_KEY) ?? undefined,
  );
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let current = true;
    void client("see your org")
      .then((routes) => routes.listOrgs())
      .then(
        ({ orgs }) => current && setOrgs(orgs),
        (error: unknown) =>
          current &&
          setProblem(`${problemText(error)} Reload the page to try again.`),
      );
    return () => {
      current = false;
    };
  }, [client, tick]);

  const choose = (id: string) => {
    sessionStorage.setItem(CHOSEN_KEY, id);
    setChosen(id);
  };
  const reload = useCallback(() => setTick((n) => n + 1), []);
  const org = orgs?.find((each) => each.id === chosen) ?? orgs?.[0];
  return { orgs, org, problem, choose, reload, version: tick };
}

export function OrgChoice({
  orgs,
  org,
  choose,
}: {
  orgs: Org[];
  org: Org;
  choose: (id: string) => void;
}) {
  if (orgs.length < 2) return null;
  return (
    <div className="payer">
      <Segmented
        label="Which org"
        value={org.id}
        onChange={choose}
        options={orgs.map((each) => ({ value: each.id, name: each.name }))}
      />
    </div>
  );
}

/** The frame every org page shares: its head, the org picker, and the states before there is an org to show. */
function OrgFrame({
  title,
  lede,
  needsToRun,
  frozenNote,
  children,
}: {
  title: string;
  lede: (org: Org) => ReactNode;
  needsToRun?: boolean;
  frozenNote?: (org: Org) => ReactNode;
  children: (org: Org, show: (text: string) => void) => ReactNode;
}) {
  const { orgs, org, problem, choose } = useOrgs();
  const { notice, show } = useNotice();
  if (problem)
    return (
      <>
        <PageHead title={title} />
        <p className="problem" role="alert">
          {problem}
        </p>
      </>
    );
  if (!orgs)
    return (
      <>
        <PageHead title={title} />
        <div className="groups">
          <Section label={title}>
            <Rows busy />
          </Section>
        </div>
      </>
    );
  if (!org)
    return (
      <PageHead title={title}>
        You are not in an org. Start one, or join one, from Team.
      </PageHead>
    );
  return (
    <>
      <PageHead title={title}>{lede(org)}</PageHead>
      <OrgChoice orgs={orgs} org={org} choose={choose} />
      {needsToRun && !runs(org) ? (
        <p className="quiet">
          Only {org.name}'s owner and admins see this. You are a{" "}
          {ROLE_WORDS[org.role]}.
        </p>
      ) : (
        <div className="groups" key={org.id}>
          {org.seats.limit === 0 && runs(org) ? (
            <div className="banner warn" role="alert">
              <IconCircleAlert size={16} />
              <div className="grow">
                {frozenNote ? (
                  frozenNote(org)
                ) : (
                  <>
                    <b>{org.name} has no Team plan running.</b>
                    <br />
                    Changes here are frozen until Team is renewed. Everything is
                    kept.
                  </>
                )}
              </div>
            </div>
          ) : null}
          {children(org, show)}
        </div>
      )}
      {notice}
    </>
  );
}

function Problem({ text }: { text?: string }) {
  return text ? (
    <p className="problem" role="alert">
      {text}
    </p>
  ) : null;
}

/** Loads what a page shows, again whenever `key` changes. */
function useLoad<T>(load: () => Promise<T>, key: string) {
  const [state, setState] = useState<{ data?: T; problem?: string }>({});
  const loadNow = useEffectEvent(load);
  useEffect(() => {
    let current = true;
    loadNow().then(
      (data) => current && setState({ data }),
      (error: unknown) =>
        current &&
        setState({
          problem: `${problemText(error)} Reload the page to try again.`,
        }),
    );
    return () => {
      current = false;
    };
  }, [key]);
  return state;
}

export function TeamsPage() {
  return (
    <OrgFrame
      title="Teams"
      lede={(org) =>
        `Named groups of ${org.name}'s members. A team holds nothing of its own; it stands in wherever a list of people goes.`
      }
    >
      {(org, show) => <Teams org={org} show={show} />}
    </OrgFrame>
  );
}

function Teams({ org, show }: { org: Org; show: (text: string) => void }) {
  const client = useClient();
  const [version, setVersion] = useState(0);
  const loaded = useLoad(async () => {
    const routes = await client("see your teams");
    const [{ teams }, { members }] = await Promise.all([
      routes.listOrgTeams(org.id),
      routes.listOrgMembers(org.id),
    ]);
    return { teams, members };
  }, `${org.id}:${version}`);
  const changed = (text: string) => {
    show(text);
    setVersion((n) => n + 1);
  };
  const frozen = org.seats.limit === 0;

  if (loaded.problem) return <Problem text={loaded.problem} />;
  return (
    <Section label="Teams" count={loaded.data?.teams.length}>
      {loaded.data ? (
        <Rows>
          {loaded.data.teams.map((team) => (
            <TeamRow
              key={team.id}
              org={org}
              team={team}
              members={loaded.data?.members ?? []}
              frozen={frozen}
              onChanged={changed}
            />
          ))}
          {runs(org) ? (
            <NewTeamRow org={org} frozen={frozen} onChanged={changed} />
          ) : loaded.data.teams.length === 0 ? (
            <ItemRow
              glyph={<IconStack size={16} />}
              name="No teams yet"
              detail="The org's owner and admins make them."
            />
          ) : null}
        </Rows>
      ) : (
        <Rows busy />
      )}
    </Section>
  );
}

function NewTeamRow({
  org,
  frozen,
  onChanged,
}: {
  org: Org;
  frozen: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [name, setName] = useState("");
  const create = () =>
    void action.run(
      async () => {
        const team = await (
          await client("make a team")
        ).createOrgTeam(org.id, { name: name.trim() });
        setName("");
        onChanged(`Made ${team.name}`);
      },
      (error) => `${problemText(error)} No team was made.`,
    );
  return (
    <ItemRow
      glyph={<IconPlus size={16} />}
      name={
        <form
          className="inline-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (name.trim()) create();
          }}
        >
          <input
            className="field"
            placeholder="New team's name"
            aria-label="New team's name"
            value={name}
            maxLength={64}
            disabled={frozen}
            onChange={(event) => setName(event.target.value)}
          />
        </form>
      }
      end={
        <button
          type="button"
          className="button small"
          onClick={create}
          disabled={frozen || !name.trim() || action.busy}
          data-busy={action.busy || undefined}
        >
          Make team
        </button>
      }
    >
      <Problem text={action.problem} />
    </ItemRow>
  );
}

function TeamRow({
  org,
  team,
  members,
  frozen,
  onChanged,
}: {
  org: Org;
  team: OrgTeam;
  members: OrgMember[];
  frozen: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [open, setOpen] = useState<"members" | "rename" | "delete">();
  const [name, setName] = useState(team.name);
  const inTeam = members.filter((member) => team.members.includes(member.id));
  const names = inTeam.map((member) => member.name ?? "Someone");
  const close = () => {
    action.clear();
    setOpen(undefined);
  };

  return (
    <ItemRow
      glyph={<IconStack size={16} />}
      name={team.name}
      detail={
        names.length === 0
          ? "Nobody yet"
          : `${names.length} · ${names.slice(0, 4).join(", ")}${names.length > 4 ? ` and ${names.length - 4} more` : ""}`
      }
      end={
        runs(org) && !open ? (
          <>
            <button
              type="button"
              className="button small"
              disabled={frozen}
              onClick={() => setOpen("members")}
            >
              Members
            </button>
            <button
              type="button"
              className="button small"
              disabled={frozen}
              onClick={() => {
                setName(team.name);
                setOpen("rename");
              }}
            >
              Rename
            </button>
            <button
              type="button"
              className="button small quiet-danger"
              disabled={frozen}
              onClick={() => setOpen("delete")}
            >
              Delete
            </button>
          </>
        ) : null
      }
    >
      {open === "members" ? (
        <div className="confirm">
          <ul className="picks">
            {members.map((member) => (
              <TeamMemberPick
                key={member.id}
                org={org}
                team={team}
                member={member}
                onChanged={onChanged}
              />
            ))}
          </ul>
          <div className="actions">
            <button type="button" className="button small" onClick={close}>
              Done
            </button>
          </div>
        </div>
      ) : open === "rename" ? (
        <Confirm
          busy={action.busy}
          problem={action.problem}
          label="Rename"
          unchanged={!name.trim() || name.trim() === team.name}
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                const renamed = await (
                  await client("rename a team")
                ).renameOrgTeam(org.id, team.id, { name: name.trim() });
                setOpen(undefined);
                onChanged(`${team.name} is now ${renamed.name}`);
              },
              (error) => `${problemText(error)} Its name is unchanged.`,
            )
          }
        >
          <input
            className="field"
            aria-label="Team name"
            value={name}
            maxLength={64}
            autoFocus
            onChange={(event) => setName(event.target.value)}
          />
        </Confirm>
      ) : open === "delete" ? (
        <Confirm
          danger
          busy={action.busy}
          problem={action.problem}
          label={`Delete ${team.name}`}
          onCancel={close}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("delete a team")
                ).deleteOrgTeam(org.id, team.id);
                setOpen(undefined);
                onChanged(`Deleted ${team.name}`);
              },
              (error) => `${problemText(error)} The team is still there.`,
            )
          }
        >
          <p>Its members stay in {org.name}; only the list goes.</p>
        </Confirm>
      ) : null}
    </ItemRow>
  );
}

function TeamMemberPick({
  org,
  team,
  member,
  onChanged,
}: {
  org: Org;
  team: OrgTeam;
  member: OrgMember;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const name = member.name ?? "Someone";
  const inTeam = team.members.includes(member.id);
  return (
    <li>
      <PersonAvatar name={member.name} imageUrl={member.imageUrl} size={20} />
      <span className="grow">
        {name}
        {member.you ? <span className="chip">You</span> : null}
      </span>
      {action.problem ? (
        <span className="problem">{action.problem}</span>
      ) : null}
      <button
        type="button"
        className={inTeam ? "button small quiet-danger" : "button small"}
        disabled={action.busy}
        data-busy={action.busy || undefined}
        onClick={() =>
          void action.run(
            async () => {
              const routes = await client("change a team");
              if (inTeam)
                await routes.removeOrgTeamMember(org.id, team.id, member.id);
              else await routes.addOrgTeamMember(org.id, team.id, member.id);
              onChanged(
                inTeam
                  ? `Took ${name} off ${team.name}`
                  : `Added ${name} to ${team.name}`,
              );
            },
            (error) => problemText(error),
          )
        }
      >
        {inTeam ? "Remove" : "Add"}
      </button>
    </li>
  );
}

const RULE_WORDS: Record<OrgRuleAdding, string> = {
  every_member: "Every member",
  admins_approve: "Admins approve first",
};

const JOINING_WORDS: Record<OrgDomainJoining, string> = {
  open: "Join on their own",
  offered: "Offered at sign-in",
};

const JOINING_DETAIL: Record<OrgDomainJoining, string> = {
  open: "people at it can join from their Team page",
  offered: "people at it are offered the org when they sign in",
};

export function DomainsPage() {
  return (
    <OrgFrame
      title="Domains"
      needsToRun
      lede={(org) =>
        `Prove ${org.name} owns an email domain, and people who sign in with a verified email there can join on a free seat, as User. Only the domain is kept, never anyone's address.`
      }
    >
      {(org, show) => <Domains org={org} show={show} />}
    </OrgFrame>
  );
}

function Domains({ org, show }: { org: Org; show: (text: string) => void }) {
  const client = useClient();
  const [version, setVersion] = useState(0);
  const loaded = useLoad(
    async () => (await client("see your domains")).listOrgDomains(org.id),
    `${org.id}:${version}`,
  );
  const changed = (text: string) => {
    show(text);
    setVersion((n) => n + 1);
  };
  const frozen = org.seats.limit === 0;

  if (loaded.problem) return <Problem text={loaded.problem} />;
  return (
    <Section label="Email domains" count={loaded.data?.domains.length}>
      {loaded.data ? (
        <Rows>
          {loaded.data.domains.map((domain) => (
            <DomainRow
              key={domain.id}
              org={org}
              domain={domain}
              frozen={frozen}
              onChanged={changed}
            />
          ))}
          <NewDomainRow org={org} frozen={frozen} onChanged={changed} />
        </Rows>
      ) : (
        <Rows busy />
      )}
    </Section>
  );
}

function NewDomainRow({
  org,
  frozen,
  onChanged,
}: {
  org: Org;
  frozen: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [domain, setDomain] = useState("");
  const [joining, setJoining] = useState<OrgDomainJoining>("offered");
  const add = () =>
    void action.run(
      async () => {
        const added = await (
          await client("add a domain")
        ).addOrgDomain(org.id, { domain: domain.trim(), joining });
        setDomain("");
        onChanged(`Added ${added.domain}. Put its TXT record in DNS next.`);
      },
      (error) => `${problemText(error)} No domain was added.`,
    );
  return (
    <ItemRow
      glyph={<IconPlus size={16} />}
      name={
        <form
          className="inline-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (domain.trim()) add();
          }}
        >
          <input
            className="field"
            placeholder="example.com"
            aria-label="Email domain"
            value={domain}
            maxLength={253}
            disabled={frozen}
            onChange={(event) => setDomain(event.target.value)}
          />
        </form>
      }
      end={
        <>
          <Segmented
            label="How people join"
            value={joining}
            onChange={setJoining}
            options={(["offered", "open"] as const).map((each) => ({
              value: each,
              name: JOINING_WORDS[each],
            }))}
          />
          <button
            type="button"
            className="button small"
            onClick={add}
            disabled={frozen || !domain.trim() || action.busy}
            data-busy={action.busy || undefined}
          >
            Add
          </button>
        </>
      }
    >
      <Problem text={action.problem} />
    </ItemRow>
  );
}

function DomainRow({
  org,
  domain,
  frozen,
  onChanged,
}: {
  org: Org;
  domain: OrgDomain;
  frozen: boolean;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [removing, setRemoving] = useState(false);
  const verified = domain.verifiedAt !== null;

  const verify = () =>
    void action.run(async () => {
      await (
        await client("verify a domain")
      ).verifyOrgDomain(org.id, domain.id);
      onChanged(`${domain.domain} is verified`);
    }, problemText);
  const setJoining = (joining: OrgDomainJoining) =>
    void action.run(
      async () => {
        await (
          await client("change how people join")
        ).setOrgDomainJoining(org.id, domain.id, { joining });
        onChanged(`${domain.domain}: ${JOINING_WORDS[joining].toLowerCase()}`);
      },
      (error) => `${problemText(error)} It is unchanged.`,
    );
  const setOurs = (ours: boolean) =>
    void action.run(
      async () => {
        await (
          await client("change whether the domain's addresses are yours")
        ).setOrgDomainOurs(org.id, domain.id, { ours });
        onChanged(
          `Addresses at ${domain.domain} ${ours ? "are" : "aren't"} ${possessive(org.name)} own`,
        );
      },
      (error) => `${problemText(error)} It is unchanged.`,
    );

  return (
    <ItemRow
      glyph={<IconGlobe size={16} />}
      name={
        <>
          {domain.domain}
          {verified ? (
            <span className="chip accent">Verified</span>
          ) : (
            <span className="chip warn">Not verified</span>
          )}
        </>
      }
      detail={
        verified
          ? `Verified ${formatDay(domain.verifiedAt ?? domain.createdAt)} · ${JOINING_DETAIL[domain.joining]}`
          : "Add this TXT record at your DNS host, then verify. Nobody can join through it until then."
      }
      end={
        removing ? null : (
          <>
            {verified ? (
              <Segmented
                label="How people join"
                value={domain.joining}
                onChange={(joining) => {
                  if (!frozen && joining !== domain.joining)
                    setJoining(joining);
                }}
                options={(["offered", "open"] as const).map((each) => ({
                  value: each,
                  name: JOINING_WORDS[each],
                }))}
              />
            ) : (
              <button
                type="button"
                className="button small primary"
                onClick={verify}
                disabled={frozen || action.busy}
                data-busy={action.busy || undefined}
              >
                Verify
              </button>
            )}
            <button
              type="button"
              className="button small quiet-danger"
              onClick={() => setRemoving(true)}
            >
              Remove
            </button>
          </>
        )
      }
    >
      {verified && !removing ? (
        <div className="row-choice">
          <div>
            <span className="name">Addresses at this domain are ours</span>
            <span className="detail">
              {domain.oursSince
                ? `Since ${formatDay(domain.oursSince)}, an address at ${domain.domain} can't sign up for its own Sikemux account or be added as a sign-in. People verify it under Work addresses instead. Accounts and sign-ins from before then stay.`
                : `Turn this on to stop new Sikemux accounts and sign-ins at ${domain.domain}. People then sign in with their own account and verify their ${domain.domain} address under Work addresses. Accounts and sign-ins that already exist stay.`}
            </span>
          </div>
          {org.role === "owner" ? (
            <Switch
              label="Addresses at this domain are ours"
              on={domain.oursSince !== null}
              disabled={frozen || action.busy}
              onChange={setOurs}
            />
          ) : (
            <span className="value">{domain.oursSince ? "On" : "Off"}</span>
          )}
        </div>
      ) : null}
      {!verified && !removing ? (
        <dl className="txt-record">
          <RecordLine label="Name" value={domain.record.name} />
          <RecordLine label="Value" value={domain.record.value} />
        </dl>
      ) : null}
      {removing ? (
        <Confirm
          danger
          busy={action.busy}
          problem={action.problem}
          label={`Remove ${domain.domain}`}
          onCancel={() => {
            action.clear();
            setRemoving(false);
          }}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("remove a domain")
                ).removeOrgDomain(org.id, domain.id);
                onChanged(`Removed ${domain.domain}`);
              },
              (error) => `${problemText(error)} It is still there.`,
            )
          }
        >
          <p>
            Nobody else can join {org.name} through it. Everyone who already did
            stays in.
          </p>
        </Confirm>
      ) : (
        <Problem text={action.problem} />
      )}
    </ItemRow>
  );
}

function RecordLine({ label, value }: { label: string; value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <>
      <dt>{label}</dt>
      <dd>
        <code>{value}</code>
        <button
          type="button"
          className="icon-button"
          aria-label={`Copy the record's ${label.toLowerCase()}`}
          onClick={() =>
            void navigator.clipboard.writeText(value).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }
        >
          <IconCopy size={13} />
        </button>
        {copied ? <span className="quiet">Copied</span> : null}
      </dd>
    </>
  );
}

export function AuditPage() {
  return (
    <OrgFrame
      title="Audit log"
      needsToRun
      lede={(org) =>
        `Who did what in ${org.name}, and when. People are named by their sign-in name only.`
      }
    >
      {(org) => <Audit org={org} />}
    </OrgFrame>
  );
}

function Audit({ org }: { org: Org }) {
  const client = useClient();
  const [entries, setEntries] = useState<OrgAuditEntry[]>();
  const [next, setNext] = useState<string | null>(null);
  const [problem, setProblem] = useState<string>();
  const more = useAction();

  useEffect(() => {
    let current = true;
    void client("see the org's log")
      .then((routes) => routes.listOrgAudit(org.id))
      .then(
        (page) => {
          if (!current) return;
          setEntries(page.entries);
          setNext(page.next);
        },
        (error: unknown) =>
          current &&
          setProblem(`${problemText(error)} Reload the page to try again.`),
      );
    return () => {
      current = false;
    };
  }, [client, org.id]);

  if (problem) return <Problem text={problem} />;
  return (
    <Section label="What happened" count={entries?.length}>
      {entries ? (
        <Rows>
          {entries.map((entry) => (
            <ItemRow
              key={entry.id}
              glyph={<AuditGlyph action={entry.action} />}
              name={auditLine(entry, org)}
              detail={new Date(entry.at).toLocaleString(undefined, {
                dateStyle: "medium",
                timeStyle: "short",
              })}
            />
          ))}
          {next ? (
            <ItemRow
              glyph={<IconActivity size={16} />}
              name="Older entries"
              end={
                <button
                  type="button"
                  className="button small"
                  disabled={more.busy}
                  data-busy={more.busy || undefined}
                  onClick={() =>
                    void more.run(async () => {
                      const page = await (
                        await client("see the org's log")
                      ).listOrgAudit(org.id, { cursor: next });
                      setEntries((shown) => [
                        ...(shown ?? []),
                        ...page.entries,
                      ]);
                      setNext(page.next);
                    }, problemText)
                  }
                >
                  Show more
                </button>
              }
            >
              <Problem text={more.problem} />
            </ItemRow>
          ) : null}
        </Rows>
      ) : (
        <Rows busy />
      )}
    </Section>
  );
}

function AuditGlyph({ action }: { action: OrgAuditEntry["action"] }) {
  if (action.startsWith("team_")) return <IconStack size={16} />;
  if (action.startsWith("domain_")) return <IconGlobe size={16} />;
  if (action === "policy_changed") return <IconShield size={16} />;
  if (action.startsWith("vault_")) return <IconLock size={16} />;
  if (action.startsWith("plan_") || action === "seats_changed")
    return <IconActivity size={16} />;
  return <IconUsers size={16} />;
}

function entryWords(kind?: string) {
  return kind === "model-key" ? "a model key" : "an entry";
}

function auditLine(entry: OrgAuditEntry, org: Org): string {
  const by = entry.by ?? "Someone";
  const about = entry.about ?? "someone";
  const role = (value?: OrgRole) => (value ? ROLE_WORDS[value] : "a member");
  const team = entry.team ?? "a team";
  const domain = entry.domain ?? "a domain";
  switch (entry.action) {
    case "created":
      return `${by} made ${org.name}`;
    case "joined":
      return entry.via === "domain"
        ? `${by} joined through ${domain}, as ${role(entry.role)}`
        : `${by} joined with an invite link, as ${role(entry.role)}`;
    case "left":
      return `${by} left`;
    case "seat_moved":
      return `${by} brought their seat over from another account, as ${role(entry.role)}`;
    case "removed":
      return `${by} removed ${about}`;
    case "role_changed":
      return entry.to === "owner"
        ? `${by} handed ${about} ownership`
        : `${by} made ${about} ${role(entry.to)}${entry.from ? `, was ${role(entry.from)}` : ""}`;
    case "invite_created":
      return `${by} made an invite link for a ${role(entry.role)}`;
    case "invite_revoked":
      return `${by} revoked an invite link`;
    case "seats_changed":
      return entry.seats === undefined
        ? `${by} changed the seats`
        : `${by} set the seats to ${entry.seats}${entry.atRenewal ? " from the next renewal" : ""}`;
    case "plan_canceled":
      return `${by} canceled the Team plan`;
    case "plan_resumed":
      return `${by} kept the Team plan going`;
    case "team_created":
      return `${by} made the team ${team}`;
    case "team_renamed":
      return `${by} renamed ${entry.previousName ?? "a team"} to ${team}`;
    case "team_deleted":
      return `${by} deleted the team ${team}`;
    case "team_member_added":
      return `${by} added ${about} to ${team}`;
    case "team_member_removed":
      return `${by} took ${about} off ${team}`;
    case "domain_added":
      return `${by} added ${domain}`;
    case "domain_verified":
      return `${by} verified ${domain}`;
    case "domain_joining_changed":
      return entry.joining
        ? `${by} set ${domain} to ${JOINING_WORDS[entry.joining].toLowerCase()}`
        : `${by} changed how people join through ${domain}`;
    case "domain_ours_changed":
      return entry.ours === false
        ? `${by} said addresses at ${domain} aren't ${possessive(org.name)} own any more`
        : `${by} said addresses at ${domain} are ${possessive(org.name)} own`;
    case "domain_removed":
      return `${by} removed ${domain}`;
    case "policy_changed":
      if (entry.ruleAdding)
        return `${by} set who can add a rule to ${RULE_WORDS[entry.ruleAdding].toLowerCase()}`;
      if (entry.asking !== undefined)
        return `${by} turned teammates' agents asking ${entry.asking ? "on" : "off"}`;
      if (entry.dailyQuestions !== undefined)
        return `${by} set questions a day per person to ${entry.dailyQuestions}`;
      return `${by} changed the policy`;
    case "vault_created":
      return `${by} made ${org.name}'s vault`;
    case "vault_host_added":
      return `${by}'s computer gave ${entry.about ? `${entry.about}'s` : "a"} computer${entry.computer ? ` “${entry.computer}”` : ""} the vault's key`;
    case "vault_rotated":
      return `${by}'s computer replaced the vault's key after a computer left`;
    case "vault_entry_written":
      return `${by} saved ${entryWords(entry.entryKind)} in the vault`;
    case "vault_entry_removed":
      return `${by} removed ${entryWords(entry.entryKind)} from the vault`;
    case "vault_remade":
      return `${by}'s computer made the vault again after no computer could open it, and dropped ${entry.droppedEntries ?? 0} ${entry.droppedEntries === 1 ? "entry" : "entries"} and ${entry.droppedComputers ?? 0} ${entry.droppedComputers === 1 ? "computer's key" : "computers' keys"}`;
  }
}

export function PolicyPage() {
  return (
    <OrgFrame
      title="Policy"
      lede={(org) =>
        runs(org)
          ? `How ${possessive(org.name)} agents work together. Every member's Sikemux follows these.`
          : `How ${possessive(org.name)} agents work together. ${possessive(org.name)} admins set these; you can see them, not change them.`
      }
      frozenNote={(org) =>
        `${possessive(org.name)} Team plan has lapsed. Policy is frozen as it was and nothing is deleted; it comes back when an owner renews in Plan and billing.`
      }
    >
      {(org, show) => <Policy org={org} show={show} />}
    </OrgFrame>
  );
}

const possessive = (name: string) =>
  name.endsWith("s") ? `${name}'` : `${name}'s`;

const DAILY_QUESTIONS = { min: 1, max: 500 };

function Policy({ org, show }: { org: Org; show: (text: string) => void }) {
  const client = useClient();
  const action = useAction();
  const loaded = useLoad(
    async () => (await client("see the org's policy")).getOrgPolicy(org.id),
    org.id,
  );
  const [policy, setPolicy] = useState<OrgPolicy>();
  const shown = policy ?? loaded.data;
  const [typed, setTyped] = useState<string>();
  const editable = runs(org) && org.seats.limit > 0;

  const change = (next: OrgPolicy, said: string) =>
    void action.run(
      async () => {
        setPolicy(
          await (
            await client("change the org's policy")
          ).setOrgPolicy(org.id, next),
        );
        setTyped(undefined);
        show(said);
      },
      (error) => `${problemText(error)} It is unchanged.`,
    );

  const saveQuestions = () => {
    if (!shown || typed === undefined) return;
    const count = Number(typed);
    if (count === shown.dailyQuestions) return setTyped(undefined);
    if (
      !Number.isInteger(count) ||
      count < DAILY_QUESTIONS.min ||
      count > DAILY_QUESTIONS.max
    ) {
      setTyped(undefined);
      return show(
        `Questions a day must be a whole number from ${DAILY_QUESTIONS.min} to ${DAILY_QUESTIONS.max}.`,
      );
    }
    change(
      { ...shown, dailyQuestions: count },
      `Questions a day per person: ${count}`,
    );
  };

  if (loaded.problem) return <Problem text={loaded.problem} />;
  if (!shown)
    return (
      <Section label="Project rules">
        <Rows busy />
      </Section>
    );
  return (
    <>
      <Section label="Project rules">
        <Rows>
          <PolicyRow
            frozen={runs(org) && !editable}
            name="Who can add a rule"
            detail={`To a project ${org.name} shares. With approval, a member's rule waits for an admin.`}
            end={
              runs(org) ? (
                <Segmented
                  label="Who can add a rule"
                  value={shown.ruleAdding}
                  onChange={(ruleAdding) => {
                    if (editable && ruleAdding !== shown.ruleAdding)
                      change(
                        { ...shown, ruleAdding },
                        `Who can add a rule: ${RULE_WORDS[ruleAdding].toLowerCase()}`,
                      );
                  }}
                  options={(["every_member", "admins_approve"] as const).map(
                    (each) => ({ value: each, name: RULE_WORDS[each] }),
                  )}
                />
              ) : (
                <span className="value">{RULE_WORDS[shown.ruleAdding]}</span>
              )
            }
          />
        </Rows>
      </Section>
      <Section label="Asking">
        <Rows>
          <PolicyRow
            frozen={runs(org) && !editable}
            name="Teammates' agents can ask yours about a task"
            detail="A project's owner can still stop anyone asking about it."
            end={
              runs(org) ? (
                <Switch
                  label="Teammates' agents can ask yours about a task"
                  on={shown.asking}
                  disabled={!editable || action.busy}
                  onChange={(asking) =>
                    change(
                      { ...shown, asking },
                      `Teammates' agents asking: ${asking ? "on" : "off"}`,
                    )
                  }
                />
              ) : (
                <span className="value">{shown.asking ? "On" : "Off"}</span>
              )
            }
          />
          <PolicyRow
            frozen={runs(org) && !editable}
            name="Questions a day per person"
            detail={`How many one person's agents may ask, across all of ${possessive(org.name)} shared projects.`}
            end={
              runs(org) ? (
                <input
                  className="field count-field"
                  type="number"
                  inputMode="numeric"
                  aria-label="Questions a day per person"
                  min={DAILY_QUESTIONS.min}
                  max={DAILY_QUESTIONS.max}
                  disabled={!editable || action.busy}
                  value={typed ?? String(shown.dailyQuestions)}
                  onChange={(event) => setTyped(event.target.value)}
                  onBlur={saveQuestions}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") event.currentTarget.blur();
                    if (event.key === "Escape") setTyped(undefined);
                  }}
                />
              ) : (
                <span className="value">{shown.dailyQuestions}</span>
              )
            }
          />
        </Rows>
      </Section>
      <Problem text={action.problem} />
    </>
  );
}

function PolicyRow({
  name,
  detail,
  end,
  frozen,
}: {
  name: string;
  detail: string;
  end: ReactNode;
  frozen: boolean;
}) {
  return (
    <li className="row choice" data-frozen={frozen || undefined}>
      <span className="name">{name}</span>
      <span className="detail">{detail}</span>
      <span className="end">{end}</span>
    </li>
  );
}

/** The orgs whose proven domain matches the person's verified email, with a way in. */
export function useOffers(ready: boolean) {
  const client = useClient();
  const [offers, setOffers] = useState<OrgOffer[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void client("see orgs you can join")
      .then((routes) => routes.listOrgOffers())
      .then(
        ({ offers }) => current && setOffers(offers),
        () => undefined,
      );
    return () => {
      current = false;
    };
  }, [client, ready, tick]);
  const reload = useCallback(() => setTick((n) => n + 1), []);
  return { offers, reload };
}

export function OfferRow({
  offer,
  sitting,
  onJoined,
}: {
  offer: OrgOffer;
  sitting?: string;
  onJoined: (org: Org) => void;
}) {
  const client = useClient();
  const action = useAction();
  return (
    <ItemRow
      glyph={<IconGlobe size={16} />}
      name={offer.name}
      detail={
        offer.seatFree
          ? `Open to everyone at ${offer.domain}. You'd join as User, on one of its free seats.`
          : `Open to everyone at ${offer.domain}, but every seat is taken. Ask its owner to add one.`
      }
      end={
        <button
          type="button"
          className="button small primary"
          disabled={!offer.seatFree || action.busy}
          data-busy={action.busy || undefined}
          onClick={() =>
            void action.run(
              async () =>
                onJoined(
                  await (
                    await client("join the org")
                  ).joinOrgByDomain({ orgId: offer.orgId, sitting }),
                ),
              (error) => `${problemText(error)} You did not join.`,
            )
          }
        >
          Join
        </button>
      }
    >
      <Problem text={action.problem} />
    </ItemRow>
  );
}

const DISMISSED_KEY = "sikemux.offersDismissed";

/** Offers an org at sign-in when its admin chose to, until the person joins or waves it off. */
export function OfferBanner({
  offers,
  onOpen,
}: {
  offers: OrgOffer[];
  onOpen: () => void;
}) {
  const [dismissed, setDismissed] = useState<string[]>(() => {
    try {
      return JSON.parse(
        localStorage.getItem(DISMISSED_KEY) ?? "[]",
      ) as string[];
    } catch {
      return [];
    }
  });
  const offer = offers.find(
    (each) =>
      each.joining === "offered" &&
      each.seatFree &&
      !dismissed.includes(each.orgId),
  );
  if (!offer) return null;
  const dismiss = () => {
    const next = [...dismissed, offer.orgId];
    localStorage.setItem(DISMISSED_KEY, JSON.stringify(next));
    setDismissed(next);
  };
  return (
    <div className="banner offer" role="status">
      <IconUsers size={16} />
      <div className="grow">
        <b>{offer.name} is on Sikemux.</b>
        <br />
        Your sign-in email is at {offer.domain}, so you can join it on a free
        seat.
      </div>
      <button type="button" className="button small" onClick={dismiss}>
        Not now
      </button>
      <button type="button" className="button small primary" onClick={onOpen}>
        See it
      </button>
    </div>
  );
}
