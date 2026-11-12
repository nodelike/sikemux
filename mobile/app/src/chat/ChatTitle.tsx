import { StyleSheet, Text } from 'react-native';

import type { AgentGroup, TurnPhase } from '@shared/wire.generated';
import { AgentFace } from '@/devices/Buddy';
import { fonts, type Palette, useStyles } from '@/ui/theme';

/**
 * A chat's header: its agent's mark or buddy, its title, then its name dimmed, as the Mac's header reads; the same
 * before and after the chat starts.
 */
export function ChatTitle({
  agentId,
  group,
  turn,
  provider,
  name,
  title,
}: {
  agentId?: string;
  group?: AgentGroup;
  /** How its turn stands, which the face wears in place of `group` where the core says it. */
  turn?: TurnPhase;
  provider: string;
  name?: string | null;
  title: string;
}) {
  const styles = useStyles(makeStyles);
  return (
    <>
      <AgentFace agentId={agentId} provider={provider} group={group} turn={turn} size={17} />
      <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
        {title}
      </Text>
      {name ? <Text style={styles.name}>@{name}</Text> : null}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    title: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink, flexShrink: 1 },
    name: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
  });
};
