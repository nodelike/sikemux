import type { Org, OrgAuditEntry, OrgRole } from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconActivity,
  IconGlobe,
  IconLock,
  IconShieldCheck,
  IconStack,
  IconUsers,
} from "@shared/ui/Icons";
import { useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { JOINING_WORDS } from "./Domains.tsx";
import { iconMark } from "./marks.ts";
import { possessive, ROLE_WORDS, RULE_WORDS, useLoad } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import {
  Btn,
  Section,
  Empty,
  Page,
  Problem,
  Row,
  Rows,
  Tile,
  useAction,
} from "./ui.tsx";

const DAYS = 14;
const PAGES = 5;

const dayStart = (back: number) => {
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  day.setDate(day.getDate() - back);
  return day.getTime();
};

export function AuditLogPage({ org }: { org: Org }) {
  const client = useClient();
  const more = useAction();
  const [older, setOlder] = useState<{
    entries: OrgAuditEntry[];
    next?: string | null;
  }>();
  const loaded = useLoad(async () => {
    const routes = await client("see the org's log");
    const entries: OrgAuditEntry[] = [];
    let next: string | null | undefined;
    for (let page = 0; page < PAGES; page += 1) {
      const got = await routes.listOrgAudit(
        org.id,
        next ? { cursor: next } : {},
      );
      entries.push(...got.entries);
      next = got.next;
      const last = entries[entries.length - 1];
      if (!next || !last || Date.parse(last.at) < dayStart(DAYS - 1)) break;
    }
    return { entries, next };
  }, org.role);

  const scene = orgScene(org.id);
  const mark = iconMark("scroll");
  if (loaded.problem)
    return (
      <Page
        title="The log didn't load"
        lede="Entries are still being written."
        actions={<Btn onClick={loaded.again}>Try again</Btn>}
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Section label="Today">
          <Problem>{loaded.problem}</Problem>
        </Section>
      </Page>
    );

  const entries = loaded.data && [
    ...loaded.data.entries,
    ...(older?.entries ?? []),
  ];
  const next = older ? older.next : loaded.data?.next;
  if (entries?.length === 0)
    return (
      <Page
        title="Audit log"
        lede="Who did what, as it happens."
        mark={mark}
        scene={scene}
      >
        <Section label="Today">
          <Empty>
            Nothing has happened yet. The first invite will be the first line.
          </Empty>
        </Section>
      </Page>
    );

  return (
    <Page
      title="Audit log"
      lede={`What happened in ${org.name}, newest first.`}
      mark={mark}
      scene={scene}
    >
      {!entries ? (
        <Section label="Today" busy />
      ) : (
        <>
          <Section label="Last 14 days">
            <Strip entries={entries} />
          </Section>
          {byDay(entries).map(([day, list]) => (
            <Section key={day} label={dayName(day)} count={list.length}>
              <Rows>
                {list.map((entry) => (
                  <Row
                    key={entry.id}
                    lead={
                      <Tile>
                        <Glyph action={entry.action} />
                      </Tile>
                    }
                    name={entry.by ?? "Someone"}
                    detail={auditLine(entry, org)}
                    end={
                      <time dateTime={entry.at}>
                        {new Date(entry.at).toLocaleTimeString(undefined, {
                          timeStyle: "short",
                        })}
                      </time>
                    }
                  />
                ))}
              </Rows>
            </Section>
          ))}
          {next ? (
            <div className="after">
              <Btn
                quiet
                busy={more.busy}
                onClick={() =>
                  void more.run(async () => {
                    const page = await (
                      await client("see the org's log")
                    ).listOrgAudit(org.id, { cursor: next });
                    setOlder((shown) => ({
                      entries: [...(shown?.entries ?? []), ...page.entries],
                      next: page.next,
                    }));
                  }, problemText)
                }
              >
                Older entries
              </Btn>
              <Problem>{more.problem}</Problem>
            </div>
          ) : null}
        </>
      )}
    </Page>
  );
}

function Strip({ entries }: { entries: OrgAuditEntry[] }) {
  const counts = Array.from({ length: DAYS }, (_, at) => {
    const back = DAYS - 1 - at;
    const from = dayStart(back);
    const to = dayStart(back - 1);
    return entries.filter((entry) => {
      const when = Date.parse(entry.at);
      return when >= from && when < to;
    }).length;
  });
  const total = counts.reduce((sum, each) => sum + each, 0);
  return (
    <div
      className="strip"
      role="img"
      aria-label={`${total} ${total === 1 ? "entry" : "entries"} in the last ${DAYS} days`}
    >
      {counts.map((count, at) => {
        const lit = Math.min(5, count);
        return (
          <span key={at}>
            {Array.from({ length: 5 }, (_, dot) =>
              dot < 5 - lit ? <i key={dot} /> : <b key={dot} />,
            )}
          </span>
        );
      })}
    </div>
  );
}

function byDay(entries: OrgAuditEntry[]): [number, OrgAuditEntry[]][] {
  const days = new Map<number, OrgAuditEntry[]>();
  for (const entry of entries) {
    const day = new Date(entry.at).setHours(0, 0, 0, 0);
    days.set(day, [...(days.get(day) ?? []), entry]);
  }
  return [...days];
}

const dayName = (day: number) =>
  day === dayStart(0)
    ? "Today"
    : day === dayStart(1)
      ? "Yesterday"
      : formatDay(new Date(day).toISOString(), "long");

function Glyph({ action }: { action: OrgAuditEntry["action"] }) {
  if (action.startsWith("team_")) return <IconStack size={14} />;
  if (action.startsWith("domain_")) return <IconGlobe size={14} />;
  if (action === "policy_changed") return <IconShieldCheck size={14} />;
  if (action.startsWith("vault_")) return <IconLock size={14} />;
  if (action.startsWith("plan_") || action === "seats_changed")
    return <IconActivity size={14} />;
  return <IconUsers size={14} />;
}

const entryWords = (kind?: string) =>
  kind === "model-key" ? "a model key" : "an entry";

/** What an entry's actor did, read after their name. */
function auditLine(entry: OrgAuditEntry, org: Org): string {
  const about = entry.about ?? "someone";
  const role = (value?: OrgRole) => (value ? ROLE_WORDS[value] : "a member");
  const group = entry.team ?? "a group";
  const domain = entry.domain ?? "a domain";
  switch (entry.action) {
    case "created":
      return `made ${org.name}`;
    case "renamed":
      return `renamed ${entry.previousName ?? "the org"} to ${entry.name ?? org.name}`;
    case "logo_changed":
      return "set the logo";
    case "logo_removed":
      return "removed the logo";
    case "joined":
      return entry.via === "domain"
        ? `joined through ${domain}, as ${role(entry.role)}`
        : `joined with an invite link, as ${role(entry.role)}`;
    case "left":
      return "left";
    case "seat_moved":
      return `brought their seat over from another account, as ${role(entry.role)}`;
    case "removed":
      return `removed ${about}`;
    case "role_changed":
      return entry.to === "owner"
        ? `handed ${about} ownership`
        : `made ${about} ${role(entry.to)}${entry.from ? `, was ${role(entry.from)}` : ""}`;
    case "invite_created":
      return `made an invite link for a ${role(entry.role)}`;
    case "invite_revoked":
      return "revoked an invite link";
    case "seats_changed":
      return entry.seats === undefined
        ? "changed the seats"
        : `set the seats to ${entry.seats}${entry.atRenewal ? " from the next renewal" : ""}`;
    case "plan_canceled":
      return "canceled the Team plan";
    case "plan_resumed":
      return "kept the Team plan going";
    case "team_created":
      return `made the group ${group}`;
    case "team_renamed":
      return `renamed ${entry.previousName ?? "a group"} to ${group}`;
    case "team_deleted":
      return `deleted the group ${group}`;
    case "team_member_added":
      return `added ${about} to ${group}`;
    case "team_member_removed":
      return `took ${about} off ${group}`;
    case "domain_added":
      return `added ${domain}`;
    case "domain_verified":
      return `verified ${domain}`;
    case "domain_joining_changed":
      return entry.joining
        ? `set ${domain} to ${JOINING_WORDS[entry.joining].toLowerCase()}`
        : `changed how people join through ${domain}`;
    case "domain_ours_changed":
      return entry.ours === false
        ? `said addresses at ${domain} aren't ${possessive(org.name)} own any more`
        : `said addresses at ${domain} are ${possessive(org.name)} own`;
    case "domain_removed":
      return `removed ${domain}`;
    case "policy_changed":
      if (entry.ruleAdding)
        return `set who can add a rule to ${RULE_WORDS[entry.ruleAdding].toLowerCase()}`;
      if (entry.asking !== undefined)
        return `turned teammates' agents asking ${entry.asking ? "on" : "off"}`;
      if (entry.dailyQuestions !== undefined)
        return `set questions a day per person to ${entry.dailyQuestions}`;
      return "changed the policy";
    case "vault_created":
      return `made ${possessive(org.name)} vault`;
    case "vault_host_added":
      return `let ${entry.about ? possessive(entry.about) : "a"} computer${entry.computer ? ` “${entry.computer}”` : ""} open the vault, from their own computer`;
    case "vault_rotated":
      return "replaced the vault's key from their computer after a computer left";
    case "vault_entry_written":
      return `saved ${entryWords(entry.entryKind)} in the vault`;
    case "vault_entry_removed":
      return `removed ${entryWords(entry.entryKind)} from the vault`;
    case "vault_remade":
      return `made the vault again from their computer after no computer could open it, and dropped ${entry.droppedEntries ?? 0} ${entry.droppedEntries === 1 ? "entry" : "entries"} and ${entry.droppedComputers ?? 0} ${entry.droppedComputers === 1 ? "computer's key" : "computers' keys"}`;
  }
}
