import type { ReactNode } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { formatAge } from '@shared/lib/format';
import { ANSWERED, cameOf, headline, NO_CALLS } from '@shared/calls/words';
import { useT } from '@/i18n';
import type { PersonCall, ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { AskerMark } from '@/calls/AskerMark';
import { openCalls, settledCalls } from '@/calls/hostCalls';
import { Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * The calls a host puts to the person, in the project in view or in every project: those open, each its question with
 * the project and how long it has waited, then those answered lately, with what came of each. An open call's row opens
 * it to be answered, and an answered one's its asker's chat.
 */
export function CallsTab({ core, snapshot, scope }: { core: string; snapshot: Snapshot; scope?: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const t = useT();
  const chatless = (call: PersonCall) => !snapshot.chats.some((chat) => chat.agentId === call.asker.agentId);
  const open = openCalls(snapshot, scope);
  const settled = settledCalls(snapshot, scope);
  const projectName = (call: PersonCall) =>
    snapshot.workspace.projects.find((project) => project.path === call.project)?.name ?? call.project.split('/').pop() ?? call.project;
  const row = (call: PersonCall, end: ReactNode) => (
    <Row
      key={call.id}
      mark={<AskerMark agentId={call.asker.agentId} provider={call.asker.cli} chief={call.asker.chief} asking={call.state === 'open'} />}
      title={headline(call, t)}
      end={end}
      onPress={
        call.state === 'open'
          ? () => router.push(`/device/${core}/calls?call=${call.id}`)
          : chatless(call)
            ? undefined
            : () => router.push(`/device/${core}/chat/${call.asker.agentId}`)
      }
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
                <View style={styles.project}>
                  <Icon name="IconFolder" size={11} color={colors.live} />
                  <Text style={styles.end} numberOfLines={1}>
                    {projectName(call)}
                  </Text>
                </View>
                <Text style={[styles.end, { color: colors.warn }]}>{formatAge(call.raisedAt, minute * 60_000, t)}</Text>
              </>,
            ),
          )}
        </Rows>
      ) : (
        <Text style={styles.none}>{t(NO_CALLS)}</Text>
      )}
      {settled.length ? (
        <>
          <SectionLabel>{t(ANSWERED)}</SectionLabel>
          <Rows>
            {settled.map((call) =>
              row(
                call,
                <Text style={styles.end} numberOfLines={1}>
                  {cameOf(call, t)}
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
    project: { flexDirection: 'row', alignItems: 'center', gap: 4 },
    none: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingVertical: 18, paddingHorizontal: 4 },
  });
};
