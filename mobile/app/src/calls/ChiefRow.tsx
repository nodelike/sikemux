import { useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { clockLabel } from '@shared/chat/messageMeta';
import { CHIEF, CHIEF_OFFLINE, CHIEF_ROLE, TAKE_OVER, chiefAway } from '@shared/calls/words';
import type { AgentShown, ProjectInfo } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { TurnMark, Working } from '@/devices/AgentMarks';
import { connectionOf, liveOf, useDevices } from '@/devices/hub';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { Row } from '@/ui/list';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * A project's chief, @Chief, first among its agents: drawn from the project itself, whether or not its chat exists
 * yet. A tap opens its chat, or asks the host for it, which makes it the first time and brings a closed one back.
 * A chief that lives on another host is tagged with that host and opens there, and while that host is out of reach
 * the row offers to take the chief over on this one.
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
  const { devices } = useDevices();
  const home = project.chiefHome;
  const away = home?.awaySince ?? undefined;
  const full = (devices.find((paired) => paired.core === core)?.access ?? 'full') === 'full';
  const homeTag =
    home &&
    (home.awaySince == null
      ? home.hostName
      : t('{host} · out of reach since {at}', { host: home.hostName, at: clockLabel(home.awaySince, now, undefined, t) }));
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

  const open = () => {
    if (shown) {
      router.push(`/device/${core}/chat/${shown.agentId}`);
      return;
    }
    if (home) {
      if (devices.some((device) => device.core === home.host)) {
        router.push(`/device/${home.host}/chat/${home.agentId}`);
      } else {
        setRefused(t('@Chief lives on {host}. Pair this phone with {host} to open its chat.', { host: home.hostName }));
      }
      return;
    }
    void ask('openChief');
  };

  const ask = async (method: 'openChief' | 'takeOverChief') => {
    if (opening) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) {
      setRefused(t(CHIEF_OFFLINE));
      return;
    }
    setOpening(true);
    setRefused(undefined);
    try {
      const opened = await callHost(connection, method, { project: project.id });
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
      host={homeTag}
      aside={t(CHIEF_ROLE)}
      detail={refused ?? (away != null ? chiefAway(away, now, t) : undefined)}
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
          {opening ? (
            <Working />
          ) : away != null && full ? (
            <Pressable onPress={() => void ask('takeOverChief')} style={styles.takeOver} accessibilityRole="button">
              <Text style={styles.takeOverText}>{t(TAKE_OVER)}</Text>
            </Pressable>
          ) : shown ? (
            <TurnMark turn={shown.turn} now={now} />
          ) : null}
        </View>
      }
      onPress={open}
    />
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    end: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    project: { flexDirection: 'row', alignItems: 'center', gap: 4, maxWidth: 120 },
    projectText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    takeOver: {
      height: 28,
      paddingHorizontal: 10,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      justifyContent: 'center',
    },
    takeOverText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.ink },
  });
};
