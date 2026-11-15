import { createContext, Fragment, useContext, useState, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import type { DeviceView, TurnState } from '@shared/wire.generated';
import { AgentFace } from '@/devices/Buddy';
import { HostContext } from '@/chat/hostFiles';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** One of the host's open agents as the transcript names it. */
export interface Named {
  id: string;
  /** What prose writes for it, such as `@Otter`. */
  written: string;
  chief: boolean;
  provider: string;
}

/** The host's open agents, longest name first so `@Otterly` is not read as `@Otter`. */
export type Crew = readonly Named[];

export const CrewContext = createContext<Crew>([]);

export const useCrew = () => useContext(CrewContext);

export function crewOf(view: DeviceView | undefined): Crew {
  if (!view) return [];
  return view.inbox.agents
    .flatMap((agent) => {
      const name = agent.chief ? 'Chief' : agent.name;
      return name ? [{ id: agent.agentId, written: `@${name}`, chief: agent.chief === true, provider: agent.cli }] : [];
    })
    .sort((a, b) => b.written.length - a.written.length);
}

const crewKey = (crew: Crew) => crew.map((agent) => `${agent.id}|${agent.written}|${agent.provider}|${agent.chief}`).join('\n');

/** The host's crew, kept as the same array while the host's views leave it as it was, so chips are not redrawn. */
export function useCrewOf(view: DeviceView | undefined): Crew {
  const fresh = crewOf(view);
  const [kept, setKept] = useState(fresh);
  if (crewKey(fresh) !== crewKey(kept)) setKept(fresh);
  return kept;
}

export const namedAgent = (crew: Crew, id: string | undefined) => (id ? crew.find((agent) => agent.id === id) : undefined);

const NAME_GOES_ON = /[\p{L}\p{N}_-]/u;

/** `text` cut where it names an agent: the words between, and each agent it names. */
export function splitNamed(text: string, crew: Crew): (string | Named)[] {
  if (!crew.length || !text.includes('@')) return [text];
  const out: (string | Named)[] = [];
  let plain = '';
  let at = 0;
  while (at < text.length) {
    const next = text.indexOf('@', at);
    if (next < 0) break;
    plain += text.slice(at, next);
    const found = crew.find(
      (agent) => text.startsWith(agent.written, next) && !NAME_GOES_ON.test(text.charAt(next + agent.written.length)),
    );
    if (found) {
      if (plain) out.push(plain);
      plain = '';
      out.push(found);
      at = next + found.written.length;
    } else {
      plain += '@';
      at = next + 1;
    }
  }
  plain += text.slice(at);
  if (plain) out.push(plain);
  return out;
}

/** The agents `agentId` works under, as the Mac's header names them: its project's chief, then the agent that started it. */
export function crumbsOf(crew: Crew, agentId: string | undefined, project: string | undefined, view: DeviceView | undefined): Named[] {
  const chief = view?.workspace.projects.find((known) => known.id === project)?.chief;
  const head = chief && chief !== agentId ? namedAgent(crew, chief) : undefined;
  if (!view || !head) return [];
  const lead = view.inbox.agents.find((agent) => agent.agentId === agentId)?.lead;
  const led = lead && lead !== chief ? namedAgent(crew, lead) : undefined;
  return led ? [head, led] : [head];
}

/** An agent's own mark: Sikemux's S on a project's chief, else the face the host gave it, else its CLI's logo. */
export function OwnMark({
  agentId,
  provider,
  chief,
  turn,
  size,
  still,
}: {
  agentId?: string | null;
  provider: string;
  chief?: boolean;
  turn?: TurnState;
  size: number;
  still?: boolean;
}) {
  const colors = useColors();
  return chief ? (
    <View style={{ width: size, height: size, alignItems: 'center', justifyContent: 'center' }}>
      <Icon name="Logo" size={size * 0.78} color={colors.accent} />
    </View>
  ) : (
    <AgentFace agentId={agentId} provider={provider} turn={turn} size={size} still={still} />
  );
}

const openChat = (core: string, agent: string) => {
  haptics.select();
  router.push(`/device/${core}/chat/${agent}`);
};

/** An agent named in prose, drawn in the line as the Mac's chip: its mark and name on a grey lift, opening its chat. */
export function NamedChip({ agent, size }: { agent: Named; size: number }) {
  const styles = useStyles(makeStyles);
  const core = useContext(HostContext);
  return (
    <Text
      style={styles.chip}
      onPress={core ? () => openChat(core, agent.id) : undefined}
      accessibilityRole={core ? 'link' : 'text'}
      accessibilityLabel={agent.written}>
      <View style={styles.inlineMark}>
        <OwnMark agentId={agent.id} provider={agent.provider} chief={agent.chief} size={size} still />
      </View>
      {` ${agent.written} `}
    </Text>
  );
}

/** Words with each agent they name by `@name` drawn as its chip. */
export function NamedWords({ text, size }: { text: string; size: number }) {
  const crew = useCrew();
  const parts = splitNamed(text, crew);
  if (parts.length === 1 && typeof parts[0] === 'string') return text;
  return parts.map((part, index) =>
    typeof part === 'string' ? <Fragment key={index}>{part}</Fragment> : <NamedChip key={index} agent={part} size={size} />,
  );
}

/** An agent's chip outside prose, as the Mac's turn line and header draw it: its mark and name on a grey lift. */
export function AgentPill({ agent, size }: { agent: Named; size: number }) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const core = useContext(HostContext);
  return (
    <Pressable
      onPress={core ? () => openChat(core, agent.id) : undefined}
      hitSlop={4}
      style={({ pressed }) => [styles.pill, { height: size + 8 }, pressed && { backgroundColor: colors.borderStrong }]}
      accessibilityRole="button"
      accessibilityLabel={agent.written}>
      <OwnMark agentId={agent.id} provider={agent.provider} chief={agent.chief} size={size + 1} still />
      <Text style={[styles.pillName, { fontSize: size }]} numberOfLines={1}>
        {agent.written}
      </Text>
    </Pressable>
  );
}

/** A trail of agents that each open their chat, every one followed by a chevron; `cut` leads it with "…". */
export function CrumbTrail({ crumbs, cut, size }: { crumbs: readonly Named[]; cut?: boolean; size: number }): ReactNode {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const chevron = <Icon name="IconChevron" size={9} color={colors.inkFaint} />;
  return (
    <View style={styles.trail}>
      {cut ? <Text style={styles.cut}>…</Text> : null}
      {crumbs.map((agent) => (
        <Fragment key={agent.id}>
          <AgentPill agent={agent} size={size} />
          {chevron}
        </Fragment>
      ))}
      {cut && !crumbs.length ? chevron : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    chip: { fontFamily: fonts.uiMedium, color: colors.ink, backgroundColor: colors.active },
    inlineMark: { paddingLeft: 3 },
    pill: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      paddingLeft: 4,
      paddingRight: 6,
      borderRadius: 5,
      backgroundColor: colors.active,
      flexShrink: 0,
    },
    pillName: { fontFamily: fonts.uiMedium, color: colors.ink },
    trail: { flexDirection: 'row', alignItems: 'center', gap: 4, flexShrink: 0 },
    cut: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
  });
