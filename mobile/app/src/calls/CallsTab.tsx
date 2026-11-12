import type { ReactNode } from 'react';
import { StyleSheet, Text } from 'react-native';
import { router } from 'expo-router';

import { formatAge } from '@shared/lib/format';
import { ANSWERED, cameOf, NO_CALLS } from '@shared/calls/words';
import type { PersonCall, ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { AskerMark } from '@/calls/AskerMark';
import { focus, openCalls, settledCalls } from '@/calls/hostCalls';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * The calls a host puts to the person, in the project in view or in every project: those open, each its question with
 * the project and how long it has waited, then those answered lately, with what came of each. A row opens the call in
 * its asker's chat, where it docks to be answered.
 */
export function CallsTab({ core, snapshot, scope }: { core: string; snapshot: Snapshot; scope?: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const open = openCalls(snapshot, scope);
  const settled = settledCalls(snapshot, scope);
  const projectName = (call: PersonCall) =>
    snapshot.workspace.projects.find((project) => project.path === call.project)?.name ?? call.project.split('/').pop() ?? call.project;
  const row = (call: PersonCall, end: ReactNode) => (
    <Row
      key={call.id}
      mark={
        <AskerMark
          agentId={call.asker.agentId}
          provider={call.asker.cli}
          chief={call.asker.chief}
          turn={call.state === 'open' ? 'needsYou' : undefined}
        />
      }
      title={call.question}
      end={end}
      onPress={() => {
        if (call.state === 'open') focus(core, call, call.asker.agentId);
        router.push(`/device/${core}/chat/${call.asker.agentId}`);
      }}
    />
  );
  return (
    <>
      {open.length ? (
        <Rows style={styles.open}>
          {open.map((call) =>
            row(
              call,
              <>
                <Text style={styles.end} numberOfLines={1}>
                  {projectName(call)}
                </Text>
                <Text style={[styles.end, { color: colors.warn }]}>{formatAge(call.raisedAt, minute * 60_000)}</Text>
              </>,
            ),
          )}
        </Rows>
      ) : (
        <Text style={styles.none}>{NO_CALLS}</Text>
      )}
      {settled.length ? (
        <>
          <SectionLabel>{ANSWERED}</SectionLabel>
          <Rows>
            {settled.map((call) =>
              row(
                call,
                <Text style={styles.end} numberOfLines={1}>
                  {cameOf(call)}
                </Text>,
              ),
            )}
          </Rows>
        </>
      ) : null}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    open: { marginTop: 10 },
    end: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, fontVariant: ['tabular-nums'], maxWidth: 140 },
    none: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingVertical: 18, paddingHorizontal: 4 },
  });
};
