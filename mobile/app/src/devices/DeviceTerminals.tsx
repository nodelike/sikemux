import { memo } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { agentLabel } from '@shared/agents/catalog';
import type { ProjectInfo, SessionInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { Working } from '@/devices/AgentMarks';
import { type Translate, useT } from '@/i18n';
import { type Palette, useColors, useStyles, useType } from '@/ui/theme';

function terminalTitle(session: SessionInfo, t: Translate): string {
  if (session.task) return session.task.label;
  if (session.agentName) return session.title ?? agentLabel(session.agentType ?? 'agent');
  return session.agentType ?? t('Terminal');
}

function terminalDetail(session: SessionInfo, t: Translate): string {
  if (session.task) return session.task.command;
  if (session.running) return t('Running');
  if (session.exit?.signal) return t('stopped by {signal}', { signal: session.exit.signal });
  return session.exit?.code != null ? t('exited {code}', { code: session.exit.code }) : t('exited');
}

const TerminalRow = memo(function TerminalRow({ session }: { session: SessionInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const t = useT();
  return (
    <Row
      dim={!session.running}
      mark={<Icon name="IconTerminal" size={15} color={session.running ? colors.live : colors.tertiary} />}
      name={session.task ? undefined : session.agentName}
      title={terminalTitle(session, t)}
      detail={<Text style={type.mono}>{terminalDetail(session, t)}</Text>}
      end={session.running ? session.task ? <Working /> : <View style={styles.liveDot} /> : null}
    />
  );
});

export function Terminals({ snapshot, scope }: { snapshot: Snapshot; scope?: ProjectInfo }) {
  const t = useT();
  const groups = new Map<string, SessionInfo[]>();
  for (const session of snapshot.sessions) {
    if (scope && session.project !== scope.id) continue;
    const project = snapshot.workspace.projects.find((known) => known.id === session.project)?.name ?? t('Other');
    groups.set(project, [...(groups.get(project) ?? []), session]);
  }
  if (!groups.size)
    return (
      <EmptyState icon="IconTerminal" text={scope ? t('No terminals in {project}.', { project: scope.name }) : t('No terminals open.')} />
    );
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
