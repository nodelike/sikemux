import { EARLIER_URI } from '@shared/wire.generated';
import { useContext, useEffect, useMemo, useState } from 'react';

import { formatAge } from '@shared/lib/format';
import type { ClosedAgent, PluginOutcome, ProjectEntry, ProjectInfo, SessionMatch, TurnState } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import { connectionOf, useLiveWhile } from '@/devices/hub';
import { translate, useT } from '@/i18n';
import type { IconName } from '@/ui/icons.generated';
import type { ContextChip } from '@/stash/words';
import { HostContext } from './hostFiles';
import { AGENT, EARLIER, FILE, FOLDER, trackedId, type Trigger, type Typed } from './mentions';
import type { ChatSession } from './session';

const SHOWN = 8;
const EARLIER_SHOWN = 4;
const EARLIER_BUDGET_MS = 600;
/** How long typing must rest before the host searches earlier chats. */
const SETTLE_MS = 150;

export type MentionMark = { agent: string | undefined; provider: string } | { icon: IconName };

/** One thing the menu offers, and what taking it writes into the draft. */
type MentionRow = {
  key: string;
  title: string;
  /** The title is words, such as a chat's, rather than a handle to type. */
  plain?: boolean;
  detail?: string | null;
  aside?: string;
  /** The heading it starts, where the rows above it belong to another. */
  group?: string;
  mark: MentionMark;
  turn?: TurnState;
  take: () => void;
};

export type MentionMenuView = {
  typed: Typed;
  rows: MentionRow[];
  /** What the menu says while it offers nothing; none while the list is on its way. */
  empty?: string;
};

type Item = { kind: 'issue' | 'pull'; number: number; title: string; url: string };
type Tracked = { items: Item[] } | { failed: string };
function read(answer: PluginOutcome, host: string): Tracked {
  if ('failed' in answer)
    return {
      failed:
        answer.failed.category === 'auth'
          ? translate('Sign in to {host} in the Git pane to pick from its issues', { host })
          : answer.failed.message,
    };
  return { items: Array.isArray(answer.value) ? (answer.value as Item[]) : [] };
}

/** The issues `#` lists or pull requests `!` lists: by the start of the number while only digits are typed, else by every word in the title. */
function trackedRows(items: Item[], trigger: Trigger, needle: string): Item[] {
  const kind = trigger === '!' ? 'pull' : 'issue';
  const query = needle.trim().toLowerCase();
  const digits = /^\d+$/.test(query);
  const words = query.split(/\s+/).filter(Boolean);
  return items
    .filter((item) => item.kind === kind)
    .filter((item) => (digits ? String(item.number).startsWith(query) : words.every((word) => item.title.toLowerCase().includes(word))))
    .slice(0, SHOWN);
}

const leaf = (path: string) => path.slice(path.lastIndexOf('/') + 1);

/**
 * What the menu over the composer offers for the word `typed` asks for: the host's other agents, closed and
 * earlier chats for `@`, the project's files and folders for `+`, its issues for `#` and pull requests for `!`.
 */
export function useMentions(session: ChatSession, typed: Typed | undefined): MentionMenuView | undefined {
  const core = useContext(HostContext);
  const t = useT();
  const live = useLiveWhile(core, typed !== undefined);
  const connection = connectionOf(live);
  const snapshot = live?.snapshot;
  const agents = snapshot?.workspace.agents;
  const own = agents?.find((agent) => agent.id === session.agentId);
  const project: ProjectInfo | undefined = snapshot?.workspace.projects.find((known) => known.id === own?.project);
  const trigger = typed?.trigger;
  const needle = typed?.needle ?? '';
  const cwd = project?.path;
  const namesAgents = trigger === '@';

  const [closed, setClosed] = useState<{ cwd: string; list: ClosedAgent[] }>();
  useEffect(() => {
    if (!namesAgents || !connection || !cwd || closed?.cwd === cwd) return;
    let current = true;
    callHost(connection, 'closedAgents', { cwd }).then(
      (list) => current && setClosed({ cwd, list }),
      () => current && setClosed({ cwd, list: [] }),
    );
    return () => {
      current = false;
    };
  }, [namesAgents, connection, cwd, closed?.cwd]);

  const [found, setFound] = useState<{ query: string; list: SessionMatch[] }>();
  useEffect(() => {
    const query = needle.trim().toLowerCase();
    if (trigger !== '@' || !connection || !cwd || !query) return;
    let current = true;
    const settle = setTimeout(() => {
      callHost(connection, 'agentSessions', { query, projects: [cwd], limit: EARLIER_SHOWN, budgetMs: EARLIER_BUDGET_MS }).then(
        (list) => current && setFound({ query, list }),
        () => current && setFound({ query, list: [] }),
      );
    }, SETTLE_MS);
    return () => {
      current = false;
      clearTimeout(settle);
    };
  }, [trigger, needle, connection, cwd]);

  const [entries, setEntries] = useState<{ query: string; list: ProjectEntry[] } | { query: string; failed: string }>();
  useEffect(() => {
    if (trigger !== '+' || !connection || !cwd) return;
    let current = true;
    callHost(connection, 'projectEntries', { project: cwd, query: needle, limit: SHOWN }).then(
      (list) => current && setEntries({ query: needle, list }),
      (error: unknown) => current && setEntries({ query: needle, failed: problemOf(error) }),
    );
    return () => {
      current = false;
    };
  }, [trigger, needle, connection, cwd]);

  const host = project?.codeHost?.label;
  const [tracked, setTracked] = useState<{ cwd: string; read: Tracked }>();
  const listsTracked = trigger === '#' || trigger === '!';
  useEffect(() => {
    if (!listsTracked || !connection || !cwd || !host || tracked?.cwd === cwd) return;
    let current = true;
    callHost(connection, 'codeHostTracked', { project: cwd }).then(
      (answer) => current && setTracked({ cwd, read: read(answer, host) }),
      (error: unknown) => current && setTracked({ cwd, read: { failed: problemOf(error) } }),
    );
    return () => {
      current = false;
    };
  }, [listsTracked, connection, cwd, host, tracked?.cwd]);

  return useMemo(() => {
    if (!typed || !snapshot) return undefined;
    const start = typed.start;
    const write = (written: string, id?: string, label?: string, context?: ContextChip) =>
      session.mention(start, { written, id, label, context });
    const wanted = typed.needle.toLowerCase();

    if (typed.trigger === '@') {
      const turns = new Map(snapshot.inbox.agents.map((agent) => [agent.agentId, agent.turn]));
      const projects = [project, ...snapshot.workspace.projects.filter((known) => known.id !== project?.id)];
      const mates: MentionRow[] = projects
        .flatMap((place) =>
          (agents ?? []).filter(
            (agent) => place && agent.project === place.id && agent.id !== session.agentId && agent.name?.toLowerCase().startsWith(wanted),
          ),
        )
        .slice(0, SHOWN)
        .map((agent) => {
          const name = agent.name ?? '';
          return {
            key: `${AGENT}${agent.id}`,
            title: `@${name}`,
            detail: agent.title,
            group:
              agent.project === project?.id ? undefined : snapshot.workspace.projects.find((known) => known.id === agent.project)?.name,
            mark: { agent: agent.id, provider: agent.provider },
            turn: turns.get(agent.id),
            take: () => write(`@${name}`, `${AGENT}${agent.id}`),
          };
        });
      const named =
        (closed?.cwd === cwd ? closed?.list : [])?.filter((agent) => agent.name.toLowerCase().startsWith(wanted)).slice(0, SHOWN) ?? [];
      const closedRows: MentionRow[] = named.map((agent, index) => ({
        key: `${EARLIER}${agent.provider}/${agent.sessionId}`,
        title: `@${agent.name}`,
        detail: agent.title,
        group: index === 0 ? t('Closed') : undefined,
        mark: { agent: undefined, provider: agent.provider },
        take: () => write(`@${agent.name}`, `${EARLIER}${agent.provider}/${agent.sessionId}`),
      }));
      const offered = new Set(snapshot.workspace.launchers.map((launcher) => launcher.provider));
      const open = new Set((agents ?? []).flatMap((agent) => (agent.resumeId ? [`${agent.provider}/${agent.resumeId}`] : [])));
      const earlier = (found?.query === wanted.trim() ? found.list : [])
        .filter((chat) => offered.has(chat.cli) && !open.has(`${chat.cli}/${chat.sessionId}`))
        .filter((chat) => !named.some((agent) => agent.sessionId === chat.sessionId));
      const earlierRows: MentionRow[] = earlier.map((chat, index) => ({
        key: `${EARLIER}${chat.cli}/${chat.sessionId}`,
        title: chat.title,
        plain: true,
        aside: chat.activeAt ? formatAge(chat.activeAt, undefined, t) : undefined,
        group: index === 0 ? t('Earlier chats') : undefined,
        mark: { agent: undefined, provider: chat.cli },
        take: () => write('', undefined, undefined, { uri: `${EARLIER_URI}${chat.cli}/${chat.sessionId}`, title: chat.title, text: '' }),
      }));
      const rows = [...mates, ...closedRows, ...earlierRows];
      return {
        typed,
        rows,
        empty: rows.length ? undefined : wanted ? t('No agent or earlier chat matches') : t('No other agent yet'),
      };
    }

    if (typed.trigger === '+') {
      if (!project) return { typed, rows: [], empty: t('Open a project to pick its files') };
      const answered = entries?.query === typed.needle ? entries : undefined;
      if (answered && 'failed' in answered) return { typed, rows: [], empty: answered.failed };
      const draft = session.draftText();
      const taken = session.snapshot().tokens.map((token) => draft.slice(token.start, token.end));
      const rows: MentionRow[] = (answered?.list ?? []).map((entry) => {
        const whole = `${project.path}/${entry.path}`;
        const name = leaf(entry.path);
        const written = `+${taken.includes(`+${name}`) ? entry.path : name}`;
        return {
          key: entry.path,
          title: name,
          detail: entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : '.',
          mark: { icon: entry.folder ? 'IconFolder' : 'IconFile' },
          take: () => write(written, `${entry.folder ? FOLDER : FILE}${whole}`),
        };
      });
      const empty = rows.length ? undefined : answered ? t('Nothing in the project matches') : undefined;
      return { typed, rows, empty };
    }

    if (!project) {
      const empty = typed.trigger === '!' ? t('Open a project to pick its pull requests') : t('Open a project to pick its issues');
      return { typed, rows: [], empty };
    }
    if (!project.codeHost) return { typed, rows: [], empty: t('This project has no remote on a code host') };
    const listed = tracked?.cwd === cwd ? tracked?.read : undefined;
    if (listed && 'failed' in listed) return { typed, rows: [], empty: listed.failed };
    const rows: MentionRow[] = trackedRows(listed?.items ?? [], typed.trigger, typed.needle).map((item) => {
      const sign = item.kind === 'pull' ? '!' : '#';
      const id = trackedId(item.kind, item.number, item.url);
      return {
        key: id,
        title: `${sign}${item.number}`,
        detail: item.title,
        mark: { icon: item.kind === 'pull' ? 'IconPullRequest' : 'IconIssue' },
        take: () => {
          write(`${sign}${item.number}`, id, `#${item.number} ${item.title}`);
          if (!connection || !cwd) return;
          callHost(connection, 'codeHostTrackedContext', { project: cwd, kind: item.kind, number: item.number }).then(
            (answer) => {
              if ('value' in answer) session.track(id, answer.value as ContextChip);
            },
            (error: unknown) => console.warn('sikemux: could not read what the issue or pull request says', error),
          );
        },
      };
    });
    const empty = rows.length
      ? undefined
      : listed
        ? typed.trigger === '!'
          ? t('No open pull request matches')
          : t('No open issue matches')
        : undefined;
    return { typed, rows, empty };
  }, [typed, snapshot, agents, project, cwd, closed, found, entries, tracked, session, connection, t]);
}
