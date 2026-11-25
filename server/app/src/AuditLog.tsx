import type { Org, OrgAuditEntry, OrgRole } from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconActivity,
  IconGlobe,
  IconLock,
  IconRefresh,
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
import "./settings.css";
import {
  Btn,
  Nothing,
  Page,
  Problem,
  Search,
  Section,
  Table,
  Td,
  Tile,
  Tr,
  useAction,
  Who,
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
  const [query, setQuery] = useState("");
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
        title="Audit log"
        lede="Every change, and who made it."
        mark={mark}
        scene={scene}
        tone="failed"
      >
        <Nothing
          title="The log didn't load"
          actions={
            <Btn quiet onClick={loaded.again}>
              <IconRefresh size={14} />
              Try again
            </Btn>
          }
        >
          {loaded.problem} Entries are still being written.
        </Nothing>
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
        lede="Every change, and who made it."
        mark={mark}
        scene={scene}
      >
        <Nothing title="Nothing has happened yet">
          The first invite will be the first line.
        </Nothing>
      </Page>
    );

  const needle = query.trim().toLowerCase();
  const rows = (entries ?? []).map((entry) => {
    const who = entry.by ?? "Someone";
    const what = auditLine(entry, org);
    const when = formatWhen(entry.at);
    return { entry, who, what, when };
  });
  const filtered = needle
    ? rows.filter(
        ({ who, what, when }) =>
          who.toLowerCase().includes(needle) ||
          what.toLowerCase().includes(needle) ||
          when.toLowerCase().includes(needle),
      )
    : rows;
  const oldest = entries?.[entries.length - 1];
  const coversWindow =
    entries !== undefined &&
    (!next ||
      (oldest !== undefined && Date.parse(oldest.at) < dayStart(DAYS - 1)));

  const loadOlder = () =>
    void more.run(async () => {
      if (!next) return;
      const page = await (
        await client("see the org's log")
      ).listOrgAudit(org.id, { cursor: next });
      setOlder((shown) => ({
        entries: [...(shown?.entries ?? []), ...page.entries],
        next: page.next,
      }));
    }, problemText);

  return (
    <Page
      title="Audit log"
      lede={`Every change in ${org.name}, newest first.`}
      tools={
        entries ? (
          <Search
            label="Find a person or change"
            value={query}
            onChange={setQuery}
          />
        ) : undefined
      }
      mark={mark}
      scene={scene}
    >
      {!entries ? (
        <Section busy />
      ) : filtered.length === 0 ? (
        <>
          <Nothing
            title={`Nothing matches “${query.trim()}”`}
            actions={
              <Btn quiet onClick={() => setQuery("")}>
                Clear
              </Btn>
            }
          >
            Try a different person or change.
          </Nothing>
          {next ? (
            <div className="aud-more">
              <Btn quiet small busy={more.busy} onClick={loadOlder}>
                {more.busy ? "Loading" : "Older entries"}
              </Btn>
              <Problem>{more.problem}</Problem>
            </div>
          ) : null}
        </>
      ) : (
        <>
          {coversWindow ? (
            <ActivityChart entries={filtered.map((row) => row.entry)} />
          ) : null}
          <div className="aud-table">
            <Table
              label="Audit log"
              cols="minmax(170px, 0.85fr) minmax(0, 2fr) minmax(130px, auto)"
              heads={["Who", "What", "When"]}
            >
              {filtered.map(({ entry, who, what, when }) => (
                <Tr key={entry.id}>
                  <Td>
                    <Who
                      lead={
                        <Tile>
                          <Glyph action={entry.action} />
                        </Tile>
                      }
                      name={who}
                      phone={what}
                    />
                  </Td>
                  <Td wide>{what}</Td>
                  <Td end>
                    <time dateTime={entry.at}>{when}</time>
                  </Td>
                </Tr>
              ))}
            </Table>
            {next ? (
              <div className="aud-more">
                <Btn quiet small busy={more.busy} onClick={loadOlder}>
                  {more.busy ? "Loading" : "Older entries"}
                </Btn>
                <Problem>{more.problem}</Problem>
              </div>
            ) : null}
          </div>
        </>
      )}
    </Page>
  );
}

function ActivityChart({ entries }: { entries: OrgAuditEntry[] }) {
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
  const max = Math.max(...counts);
  const firstLabel = formatDay(
    new Date(dayStart(DAYS - 1)).toISOString(),
    "short",
  );
  return (
    <div
      className="aud-heat"
      role="img"
      aria-label={
        total > 0
          ? `${total} ${total === 1 ? "change" : "changes"} in the last ${DAYS} days, most ${max} in one`
          : `No changes in the last ${DAYS} days`
      }
    >
      <div className="aud-bars">
        {counts.map((count, at) => (
          <i
            key={at}
            data-zero={count === 0 || undefined}
            title={`${count} ${count === 1 ? "change" : "changes"}`}
            style={{
              height: count === 0 ? 3 : Math.round(10 + (count / max) * 50),
            }}
          />
        ))}
      </div>
      <p className="aud-cap">
        <span>{firstLabel}</span>
        <span>
          {total > 0
            ? `${total} ${total === 1 ? "change" : "changes"} in ${DAYS} days, most ${max} in one`
            : `No changes in the last ${DAYS} days`}
        </span>
        <span>Today</span>
      </p>
    </div>
  );
}

function formatWhen(at: string): string {
  const date = new Date(at);
  const day = new Date(date).setHours(0, 0, 0, 0);
  const time = date.toLocaleTimeString(undefined, { timeStyle: "short" });
  if (day === dayStart(0)) return `Today, ${time}`;
  if (day === dayStart(1)) return `Yesterday, ${time}`;
  return `${formatDay(at, "short")}, ${time}`;
}

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
