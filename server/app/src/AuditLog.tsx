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
import { useState, type ReactNode } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./client.ts";
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
  Tile,
  useAction,
} from "./ui.tsx";

const DAYS = 14;
const LEDE = "Every change, and who made it.";

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
  const loaded = useLoad(
    async () => (await client("see the org's log")).listOrgAudit(org.id, {}),
    org.role,
  );

  const scene = orgScene(org.id);
  const mark = iconMark("scroll");
  if (loaded.problem)
    return (
      <Page
        title="Audit log"
        lede={LEDE}
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
      <Page title="Audit log" lede={LEDE} mark={mark} scene={scene}>
        <Nothing title="Nothing has happened yet">
          The first invite will be the first line.
        </Nothing>
      </Page>
    );

  const needle = query.trim().toLowerCase();
  const rows = (entries ?? []).map((entry) => {
    const who = entry.by ?? "Someone";
    const line = auditLine(entry, org);
    const day = new Date(entry.at).setHours(0, 0, 0, 0);
    const label = dayName(day);
    return { entry, who, line, day, label };
  });
  const filtered = needle
    ? rows.filter(
        ({ who, line, label }) =>
          who.toLowerCase().includes(needle) ||
          line.text.toLowerCase().includes(needle) ||
          label.toLowerCase().includes(needle),
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

  const days = groupByDay(filtered);

  return (
    <Page
      title="Audit log"
      lede={LEDE}
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
      ) : (
        <div className="aud">
          {filtered.length === 0 ? (
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
          ) : (
            <>
              {coversWindow ? (
                <ActivityChart entries={filtered.map((row) => row.entry)} />
              ) : null}
              {days.map(([day, label, list]) => (
                <section key={day} className="aud-day">
                  <h2>{label}</h2>
                  <ul>
                    {list.map(({ entry, who, line }) => (
                      <li key={entry.id}>
                        <Tile>
                          <Glyph action={entry.action} />
                        </Tile>
                        <span className="aud-line">
                          <b>{who}</b> {line.nodes}
                        </span>
                        <time dateTime={entry.at}>
                          {new Date(entry.at).toLocaleTimeString(undefined, {
                            timeStyle: "short",
                          })}
                        </time>
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </>
          )}
          {next ? (
            <div className="aud-more">
              <Btn quiet small busy={more.busy} onClick={loadOlder}>
                {more.busy ? "Loading" : "Older entries"}
              </Btn>
              <Problem>{more.problem}</Problem>
            </div>
          ) : null}
        </div>
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
  const firstLabel = new Date(dayStart(DAYS - 1)).toLocaleDateString(
    undefined,
    { day: "numeric", month: "short" },
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

function groupByDay<T extends { day: number; label: string }>(
  rows: T[],
): [number, string, T[]][] {
  const days = new Map<number, { label: string; list: T[] }>();
  for (const row of rows) {
    const group = days.get(row.day);
    if (group) group.list.push(row);
    else days.set(row.day, { label: row.label, list: [row] });
  }
  return [...days.entries()].map(([day, { label, list }]) => [
    day,
    label,
    list,
  ]);
}

function dayName(day: number): string {
  if (day === dayStart(0)) return "Today";
  if (day === dayStart(1)) return "Yesterday";
  const date = new Date(day);
  if (date.getFullYear() === new Date().getFullYear())
    return date.toLocaleDateString(undefined, {
      day: "numeric",
      month: "long",
    });
  return formatDay(date.toISOString(), "long");
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

type Part = string | { b: string };

const strong = (value: string): Part => ({ b: value });

function renderParts(parts: Part[]): { nodes: ReactNode; text: string } {
  return {
    nodes: parts.map((part, at) =>
      typeof part === "string" ? part : <b key={at}>{part.b}</b>,
    ),
    text: parts
      .map((part) => (typeof part === "string" ? part : part.b))
      .join(""),
  };
}

const entryWords = (kind?: string) =>
  kind === "model-key" ? "a model key" : "an entry";

/** What an entry's actor did, read after their name. */
function auditLine(
  entry: OrgAuditEntry,
  org: Org,
): { nodes: ReactNode; text: string } {
  const about = entry.about ?? "someone";
  const role = (value?: OrgRole) => (value ? ROLE_WORDS[value] : "a member");
  const group = entry.team ?? "a group";
  const domain = entry.domain ?? "a domain";
  switch (entry.action) {
    case "created":
      return renderParts(["made ", strong(org.name)]);
    case "renamed":
      return renderParts([
        "renamed ",
        entry.previousName ? strong(entry.previousName) : "the org",
        " to ",
        strong(entry.name ?? org.name),
      ]);
    case "logo_changed":
      return renderParts(["set the logo"]);
    case "logo_removed":
      return renderParts(["removed the logo"]);
    case "joined":
      return entry.via === "domain"
        ? renderParts([
            "joined through ",
            strong(domain),
            ", as ",
            role(entry.role),
          ])
        : renderParts(["joined with an invite link, as ", role(entry.role)]);
    case "left":
      return renderParts(["left"]);
    case "seat_moved":
      return renderParts([
        "brought their seat over from another account, as ",
        role(entry.role),
      ]);
    case "removed":
      return renderParts(["removed ", strong(about)]);
    case "role_changed":
      return entry.to === "owner"
        ? renderParts(["handed ", strong(about), " ownership"])
        : renderParts([
            "made ",
            strong(about),
            " ",
            strong(role(entry.to)),
            ...(entry.from ? [", was ", role(entry.from)] : []),
          ]);
    case "invite_created":
      return renderParts([
        "made an invite link for a ",
        strong(role(entry.role)),
      ]);
    case "invite_revoked":
      return renderParts(["revoked an invite link"]);
    case "seats_changed":
      return entry.seats === undefined
        ? renderParts(["changed the seats"])
        : renderParts([
            "set the seats to ",
            strong(String(entry.seats)),
            ...(entry.atRenewal ? [" from the next renewal"] : []),
          ]);
    case "plan_canceled":
      return renderParts(["canceled the Team plan"]);
    case "plan_resumed":
      return renderParts(["kept the Team plan going"]);
    case "team_created":
      return renderParts(["made the group ", strong(group)]);
    case "team_renamed":
      return renderParts([
        "renamed ",
        entry.previousName ? strong(entry.previousName) : "a group",
        " to ",
        strong(group),
      ]);
    case "team_deleted":
      return renderParts(["deleted the group ", strong(group)]);
    case "team_member_added":
      return renderParts(["added ", strong(about), " to ", strong(group)]);
    case "team_member_removed":
      return renderParts(["took ", strong(about), " off ", strong(group)]);
    case "domain_added":
      return renderParts(["added ", strong(domain)]);
    case "domain_verified":
      return renderParts(["verified ", strong(domain)]);
    case "domain_joining_changed":
      return entry.joining
        ? renderParts([
            "set ",
            strong(domain),
            " to ",
            strong(JOINING_WORDS[entry.joining].toLowerCase()),
          ])
        : renderParts(["changed how people join through ", strong(domain)]);
    case "domain_ours_changed":
      return entry.ours === false
        ? renderParts([
            "said addresses at ",
            strong(domain),
            ` aren't ${possessive(org.name)} own any more`,
          ])
        : renderParts([
            "said addresses at ",
            strong(domain),
            ` are ${possessive(org.name)} own`,
          ]);
    case "domain_removed":
      return renderParts(["removed ", strong(domain)]);
    case "policy_changed":
      if (entry.ruleAdding)
        return renderParts([
          "set who can add a rule to ",
          strong(RULE_WORDS[entry.ruleAdding].toLowerCase()),
        ]);
      if (entry.asking !== undefined)
        return renderParts([
          "turned teammates' agents asking ",
          strong(entry.asking ? "on" : "off"),
        ]);
      if (entry.dailyQuestions !== undefined)
        return renderParts([
          "set questions a day per person to ",
          strong(String(entry.dailyQuestions)),
        ]);
      return renderParts(["changed the policy"]);
    case "vault_created":
      return renderParts([`made ${possessive(org.name)} vault`]);
    case "vault_host_added":
      return renderParts([
        "let ",
        entry.about ? strong(possessive(entry.about)) : "a",
        " computer",
        ...(entry.computer ? [" “", strong(entry.computer), "”"] : []),
        " open the vault, from their own computer",
      ]);
    case "vault_rotated":
      return renderParts([
        "replaced the vault's key from their computer after a computer left",
      ]);
    case "vault_entry_written":
      return renderParts([
        "saved ",
        entryWords(entry.entryKind),
        " in the vault",
      ]);
    case "vault_entry_removed":
      return renderParts([
        "removed ",
        entryWords(entry.entryKind),
        " from the vault",
      ]);
    case "vault_remade": {
      const entriesCount = entry.droppedEntries ?? 0;
      const computersCount = entry.droppedComputers ?? 0;
      return renderParts([
        "made the vault again from their computer after no computer could open it, and dropped ",
        strong(`${entriesCount} ${entriesCount === 1 ? "entry" : "entries"}`),
        " and ",
        strong(
          `${computersCount} ${computersCount === 1 ? "computer's key" : "computers' keys"}`,
        ),
      ]);
    }
  }
}
