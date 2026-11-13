import { StyleSheet, Text } from 'react-native';

import type { TurnState } from '@shared/wire.generated';
import { AgentFace } from '@/devices/Buddy';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * A chat's header: its agent's mark or buddy, or Sikemux's S for the project's chief, its title, then its name dimmed, as the Mac's header reads; the same
 * before and after the chat starts.
 */
export function ChatTitle({
  agentId,
  turn,
  provider,
  name,
  title,
  chief,
}: {
  agentId?: string;
  /** How its turn stands, which the face wears. */
  turn?: TurnState;
  provider: string;
  name?: string | null;
  title: string;
  chief?: boolean;
}) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  return (
    <>
      {chief ? (
        <Icon name="Logo" size={13} color={colors.accent} />
      ) : (
        <AgentFace agentId={agentId} provider={provider} turn={turn} size={17} />
      )}
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
