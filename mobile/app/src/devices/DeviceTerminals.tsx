import { memo } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { agentLabel } from '@shared/agents/catalog';
import type { ProjectInfo, SessionInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { Working } from '@/devices/AgentMarks';
import { type Palette, useColors, useStyles, useType } from '@/ui/theme';

function terminalTitle(session: SessionInfo): string {
  if (session.task) return session.task.label;
  if (session.agentName) return session.title ?? agentLabel(session.agentType ?? 'agent');
  return session.agentType ?? 'Terminal';
}

function terminalDetail(session: SessionInfo): string {
  if (session.task) return session.task.command;
  if (session.running) return 'Running';
  if (session.exit?.signal) return `stopped by ${session.exit.signal}`;
  return session.exit?.code != null ? `exited ${session.exit.code}` : 'exited';
}

const TerminalRow = memo(function TerminalRow({ session }: { session: SessionInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  return (
    <Row
      dim={!session.running}
      mark={
        <Icon
          name={session.task ? 'IconRun' : 'IconCommand'}
          size={session.task ? 15 : 18}
          color={session.running ? colors.secondary : colors.tertiary}
        />
      }
      name={session.task ? undefined : session.agentName}
      title={terminalTitle(session)}
      detail={<Text style={type.mono}>{terminalDetail(session)}</Text>}
      end={session.running ? session.task ? <Working /> : <View style={styles.liveDot} /> : null}
    />
  );
});

export function Terminals({ snapshot, scope }: { snapshot: Snapshot; scope?: ProjectInfo }) {
  const groups = new Map<string, SessionInfo[]>();
  for (const session of snapshot.sessions) {
    if (scope && session.project !== scope.id) continue;
    const project = snapshot.workspace.projects.find((known) => known.id === session.project)?.name ?? 'Other';
    groups.set(project, [...(groups.get(project) ?? []), session]);
  }
  if (!groups.size) return <EmptyState icon="IconCommand" text={scope ? `No terminals in ${scope.name}.` : 'No terminals open.'} />;
  return (
    <>
      {[...groups].map(([project, sessions]) => (
        <View key={project}>
          {scope ? <View style={{ height: 8 }} /> : <SectionLabel>{project}</SectionLabel>}
          <Rows>
            {sessions.map((session) => (
              <TerminalRow key={session.id} session={session} />
            ))}
          </Rows>
        </View>
      ))}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    liveDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: colors.live },
  });
};
