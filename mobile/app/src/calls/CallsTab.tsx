import { memo, type ReactNode, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { formatAge } from '@shared/lib/format';
import { ANSWERED, cameOf, NO_CALLS } from '@shared/calls/words';
import type { PersonCall, ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import type { ConnectionLike } from '@sikemux/native';
import { AskerMark } from '@/calls/AskerMark';
import { CallDock } from '@/calls/CallDock';
import { draftOf, focus, openCalls, settledCalls } from '@/calls/hostCalls';
import { useExternalValue } from '@shared/lib/externalValue';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * The calls a host puts to the person, in the project in view or in every project: those open, each its question with
 * the project and how long it has waited, then those answered lately, with what came of each. A row opens the call in
 * its asker's chat, where it docks to be answered. An open call whose asker has no chat on this host, such as an agent
 * in a terminal or a linked host's worker, unfolds its card under its row instead.
 */
export function CallsTab({
  core,
  snapshot,
  scope,
  connection,
  canAnswer,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  connection: ConnectionLike | undefined;
  canAnswer: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const [unfolded, setUnfolded] = useState<string>();
  const chatless = (call: PersonCall) => !snapshot.chats.some((chat) => chat.agentId === call.asker.agentId);
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
      onPress={
        chatless(call)
          ? call.state === 'open'
            ? () => setUnfolded(unfolded === call.id ? undefined : call.id)
            : undefined
          : () => {
              if (call.state === 'open') focus(core, call, call.asker.agentId);
              router.push(`/device/${core}/chat/${call.asker.agentId}`);
            }
      }
    />
  );
  const card = (call: PersonCall) =>
    unfolded === call.id ? (
      <InPlace key={`${call.id}-card`} core={core} calls={snapshot.calls} call={call} connection={connection} canAnswer={canAnswer} />
    ) : null;
  return (
    <>
      {open.length ? (
        <Rows style={styles.open}>
          {open.flatMap((call) => [
            row(
              call,
              <>
                <Text style={styles.end} numberOfLines={1}>
                  {projectName(call)}
                </Text>
                <Text style={[styles.end, { color: colors.warn }]}>{formatAge(call.raisedAt, minute * 60_000)}</Text>
              </>,
            ),
            card(call),
          ])}
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

/** An open call's card under its row, with why an answer did not go under it while no other is on its way. */
const InPlace = memo(function InPlace({
  core,
  calls,
  call,
  connection,
  canAnswer,
}: {
  core: string;
  calls: readonly PersonCall[] | undefined;
  call: PersonCall;
  connection: ConnectionLike | undefined;
  canAnswer: boolean;
}) {
  const styles = useStyles(makeStyles);
  const [refused, setRefused] = useState<string>();
  const sending = useExternalValue(draftOf(calls, call)).sending;
  return (
    <View style={styles.unfolded}>
      <CallDock
        core={core}
        calls={calls}
        call={call}
        chat={call.asker.agentId}
        queue={[call]}
        connection={connection}
        canAnswer={canAnswer}
        report={setRefused}
        inPlace
      />
      {refused && !sending ? <Text style={styles.refused}>{refused}</Text> : null}
    </View>
  );
});

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    open: { marginTop: 10 },
    end: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, fontVariant: ['tabular-nums'], maxWidth: 140 },
    unfolded: { paddingVertical: 8 },
    refused: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.danger, paddingTop: 6, paddingHorizontal: 4 },
    none: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingVertical: 18, paddingHorizontal: 4 },
  });
};
