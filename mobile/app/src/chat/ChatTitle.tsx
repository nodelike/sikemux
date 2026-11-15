import { useState } from 'react';
import { type LayoutChangeEvent, StyleSheet, Text, View } from 'react-native';

import type { TurnState } from '@shared/wire.generated';
import { CrumbTrail, type Named, OwnMark } from '@/devices/Crew';
import { fonts, type Palette, useStyles } from '@/ui/theme';

/** The room the chat's own words keep beside its crumbs before the crumbs give way. */
const WORDS_KEEP = 64;
const GAP = 6;
const MARK = 17;

/**
 * A chat's header, as the Mac's reads: the agents it works under, each a chip that opens it, then its own mark, its
 * title and its name dimmed. A chat with no title of its own yet shows its name in the title's place. Short of room,
 * the crumbs give way from the left behind "…".
 */
export function ChatTitle({
  agentId,
  turn,
  provider,
  name,
  title,
  chief,
  crumbs = [],
}: {
  agentId?: string;
  /** How its turn stands, which the face wears. */
  turn?: TurnState;
  provider: string;
  name?: string | null;
  title?: string;
  chief?: boolean;
  /** The agents it works under, the chief first. */
  crumbs?: readonly Named[];
}) {
  const styles = useStyles(makeStyles);
  const [room, setRoom] = useState(0);
  const [widths, setWidths] = useState<Record<string, number>>({});
  const handle = name ? `@${name}` : chief ? '@Chief' : undefined;
  const shown = crumbsThatFit(crumbs, widths, room);
  const measure = (key: string) => (event: LayoutChangeEvent) => {
    const width = Math.ceil(event.nativeEvent.layout.width);
    setWidths((known) => (known[key] === width ? known : { ...known, [key]: width }));
  };
  return (
    <View style={styles.row} onLayout={(event) => setRoom(Math.floor(event.nativeEvent.layout.width))}>
      {crumbs.length ? (
        <>
          <View style={styles.measured} pointerEvents="none" importantForAccessibility="no-hide-descendants">
            {crumbs.map((agent, index) => (
              <View key={agent.id} onLayout={measure(`${index}`)}>
                <CrumbTrail crumbs={crumbs.slice(index)} cut={index > 0} size={12.5} />
              </View>
            ))}
            <View onLayout={measure('cut')}>
              <CrumbTrail crumbs={[]} cut size={12.5} />
            </View>
          </View>
          {shown ? <CrumbTrail crumbs={crumbs.slice(shown.from)} cut={shown.from > 0} size={12.5} /> : null}
        </>
      ) : null}
      <OwnMark agentId={agentId} provider={provider} chief={chief} turn={turn} size={MARK} />
      <View style={[styles.words, crumbs.length > 0 && { minWidth: WORDS_KEEP }]}>
        {title ? (
          <>
            <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
              {title}
            </Text>
            {handle ? <Text style={styles.name}>{handle}</Text> : null}
          </>
        ) : handle ? (
          <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
            {handle}
          </Text>
        ) : null}
      </View>
    </View>
  );
}

/** The crumbs shown from `from` on: all of them while they fit beside the words, else fewer behind "…". */
function crumbsThatFit(crumbs: readonly Named[], widths: Record<string, number>, room: number): { from: number } | undefined {
  if (!crumbs.length || !room) return undefined;
  const left = room - GAP - MARK - GAP - WORDS_KEEP;
  for (let from = 0; from < crumbs.length; from += 1) {
    const width = widths[`${from}`];
    if (width == null) return undefined;
    if (width <= left) return { from };
  }
  return { from: crumbs.length };
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    row: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: GAP },
    measured: { position: 'absolute', opacity: 0, left: 0, top: 0, width: 2000, alignItems: 'flex-start' },
    words: { flexShrink: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: GAP },
    title: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink, flexShrink: 1 },
    name: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
  });
};
