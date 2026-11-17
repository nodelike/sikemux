import { createContext, useContext } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { getCalendars } from 'expo-localization';

import { whyWords } from '@shared/chat/why';
import type { WhyChip } from '@shared/wire.generated';
import { HostContext } from '@/chat/hostFiles';
import { AgentContext } from '@/chat/transcriptContext';
import { useLanguage, useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** The `why` links the core checked in the answer being drawn. */
export const WhyChipsContext = createContext<WhyChip[] | undefined>(undefined);

/**
 * What a claim in an answer rests on, as the core checked it, drawn in the line as the Mac's chip: what it is, then
 * when. Only a chip of another chat opens, to that agent's chat; the phone has nowhere to show a commit, a rule or a call.
 */
export function WhyChipText({ chip }: { chip: WhyChip }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const locale = useLanguage();
  const core = useContext(HostContext);
  const { icon, key, title } = whyWords(chip, { say: t, locale, hour12: getCalendars()[0]?.uses24hourClock === false });
  const here = useContext(AgentContext);
  const agent = chip.open.kind === 'chat' && chip.open.agent !== here ? chip.open.agent : undefined;
  return (
    <Text
      style={styles.chip}
      onPress={agent && core ? () => router.push(`/device/${core}/chat/${agent}`) : undefined}
      accessibilityRole={agent && core ? 'link' : 'text'}>
      <View style={styles.mark}>
        <Icon name={icon} size={12} color={colors.tertiary} />
      </View>{' '}
      {key}
      {title ? <Text style={styles.title}>{` ${title}`}</Text> : null}
    </Text>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    chip: { fontFamily: fonts.ui, color: colors.ink, backgroundColor: colors.active },
    mark: { paddingLeft: 3 },
    title: { color: colors.secondary },
  });
