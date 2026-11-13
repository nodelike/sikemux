import { memo, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import type { RemoteAgent } from '@shared/wire.generated';
import { router } from 'expo-router';

import { AgentFace } from '@/devices/Buddy';
import { useDevices } from '@/devices/hub';
import { Row } from '@/ui/list';
import { LinePlace, ShellCount, SubagentCount, TurnMark } from '@/devices/AgentMarks';
import { useMinute } from '@/ui/motion';
import { type Palette, typeFor, useStyles } from '@/ui/theme';
import { useT } from '@/i18n';

/**
 * A worker an agent here started on a linked host: the row any agent has, with the host it runs on tagged. Tapping
 * opens its chat on that host when this phone is paired with it, and otherwise says where it runs.
 */
export const WorkerRow = memo(function WorkerRow({ worker }: { worker: RemoteAgent }) {
  const t = useT();
  const minute = useMinute();
  const { devices } = useDevices();
  const paired = devices.some((device) => device.core === worker.hostId);
  const reachable = worker.reachable;
  const styles = useStyles(makeStyles);
  const [told, setTold] = useState(false);
  const who = worker.name ? `@${worker.name}` : worker.title;
  return (
    <View>
      <Row
        dim={!reachable}
        mark={
          <View style={!reachable && styles.away}>
            <AgentFace agentId={worker.agentId} provider={worker.cli} turn={worker.turn} size={20} />
          </View>
        }
        title={worker.titled ? worker.title : undefined}
        name={worker.name}
        host={reachable ? worker.host : `${worker.host} · ${t('Out of reach').toLowerCase()}`}
        end={
          reachable ? (
            <>
              {worker.subagents ? <SubagentCount count={worker.subagents} /> : null}
              {worker.tasks ? <ShellCount count={worker.tasks} /> : null}
              {worker.line ? <LinePlace line={worker.line} /> : null}
              <TurnMark turn={worker.turn} now={minute * 60_000} />
            </>
          ) : undefined
        }
        onPress={paired ? () => router.push(`/device/${worker.hostId}/chat/${worker.agentId}`) : () => setTold((was) => !was)}
      />
      {told && !paired ? (
        <Text style={styles.told}>
          {t('{who} runs on {host}. Pair this phone with {host} to open its chat.', { who, host: worker.host })}
        </Text>
      ) : null}
    </View>
  );
});

export function workersOf(workers: readonly RemoteAgent[], lead: string): RemoteAgent[] {
  return workers.filter((worker) => worker.lead === lead);
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    away: { opacity: 0.55 },
    told: { ...typeFor(colors).meta, fontSize: 12.5, paddingLeft: 38, paddingRight: 8, paddingBottom: 8 },
  });
