import { useEffect, useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { CHIEF, CHIEF_OFFLINE } from '@shared/calls/words';
import type { AgentShown, ProjectInfo } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { TurnMark, Working } from '@/devices/AgentMarks';
import { connectionOf, liveOf } from '@/devices/hub';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { Row } from '@/ui/list';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * A project's chief, @Chief, first among its agents: drawn from the project itself, whether or not its chat exists
 * yet. A tap opens its chat, or asks the host for it, which makes it the first time and brings a closed one back.
 */
export function ChiefRow({
  core,
  project,
  shown,
  mixed,
  now,
}: {
  core: string;
  project: ProjectInfo;
  /** The chief's own row in the host's list, while its chat is open. */
  shown?: AgentShown;
  /** Agents of several projects share the list, so the row names its project at its end. */
  mixed: boolean;
  now: number;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [opening, setOpening] = useState(false);
  const [refused, setRefused] = useState<string>();
  // The row is on screen, so the chat it asked for may open over it.
  const here = useRef(true);
  useEffect(
    () => () => {
      here.current = false;
    },
    [],
  );

  const open = async () => {
    if (shown) {
      router.push(`/device/${core}/chat/${shown.agentId}`);
      return;
    }
    if (opening) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) {
      setRefused(t(CHIEF_OFFLINE));
      return;
    }
    setOpening(true);
    setRefused(undefined);
    try {
      const opened = await callHost(connection, 'openChief', { project: project.id });
      if (here.current) router.push(`/device/${core}/chat/${opened.agentId}`);
    } catch (error) {
      setRefused(problem(error));
    } finally {
      setOpening(false);
    }
  };

  return (
    <Row
      mark={<Icon name="Logo" size={15} color={colors.accent} />}
      title={CHIEF}
      aside={shown?.titled ? shown.title : undefined}
      detail={refused}
      end={
        <View style={styles.end}>
          {mixed ? (
            <View style={styles.project}>
              <Icon name="IconFolder" size={11} color={colors.live} />
              <Text style={styles.projectText} numberOfLines={1}>
                {project.name}
              </Text>
            </View>
          ) : null}
          {opening ? <Working /> : shown ? <TurnMark turn={shown.turn} now={now} /> : null}
        </View>
      }
      onPress={() => void open()}
    />
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    end: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    project: { flexDirection: 'row', alignItems: 'center', gap: 4, maxWidth: 120 },
    projectText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
  });
};
