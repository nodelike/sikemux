import { memo, useEffect, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text } from 'react-native';
import { router } from 'expo-router';

import { formatAge } from '@shared/lib/format';
import type { ProjectInfo, RecentInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { problem as problemOf } from '@/core/problem';
import { useLive } from '@/devices/hub';
import { projectName } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { useMinute } from '@/ui/motion';
import { AgentIcon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { Working } from '@/devices/AgentMarks';
import { type Palette, typeFor, useStyles, useType } from '@/ui/theme';

/** Whether something on the host is in the project and for the agent the screen shows. */
export function listed(
  scope: ProjectInfo | undefined,
  provider: string,
  project: string | null | undefined,
  type: string | null | undefined,
): boolean {
  return (!scope || project === scope.id) && (provider === 'all' || type === provider);
}

/** Recent chats shown before Show more. */
const RECENT_SHOWN = 10;

/** Saved chats the host's rail lists as recent; a phone with full access takes one up again. */
export function Recent({
  core,
  snapshot,
  scope,
  provider,
  full,
  hostName,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  provider: string;
  full: boolean;
  hostName: string;
}) {
  const styles = useStyles(makeStyles);
  const live = useLive(core);
  const minute = useMinute();
  const [all, setAll] = useState(false);
  const [resuming, setResuming] = useState<string>();
  const [note, setNote] = useState<{ text: string; failed: boolean }>();
  const chats = snapshot.recent.filter((chat) => !chat.agentId && listed(scope, provider, chat.project, chat.provider));
  const shown = all ? chats : chats.slice(0, RECENT_SHOWN);

  const resume = async (chat: RecentInfo) => {
    if (resuming) return;
    if (!full) {
      haptics.warning();
      setNote({ text: `This phone can only watch ${hostName}.`, failed: false });
      return;
    }
    if (live.status !== 'open') {
      haptics.failure();
      setNote({ text: 'Not connected to the host.', failed: true });
      return;
    }
    haptics.tap();
    setNote(undefined);
    setResuming(chat.id);
    try {
      const agentId = await live.connection.resumeChat(chat.id);
      router.push(`/device/${core}/chat/${agentId}`);
    } catch (error) {
      haptics.failure();
      setNote({ text: problemOf(error), failed: true });
    } finally {
      setResuming(undefined);
    }
  };

  // The rows only redraw when their own chat does, so they call the latest resume through a ref.
  const latestResume = useRef(resume);
  useEffect(() => {
    latestResume.current = resume;
  });
  const onResume = (chat: RecentInfo) => void latestResume.current(chat);

  if (!chats.length) return null;
  return (
    <>
      <SectionLabel>Recent</SectionLabel>
      <Rows>
        {shown.map((chat) => (
          <RecentRow
            key={chat.id}
            chat={chat}
            place={scope ? undefined : projectName(snapshot, chat.project, chat.cwd)}
            resuming={resuming === chat.id}
            minute={minute}
            onResume={onResume}
          />
        ))}
        {shown.length < chats.length ? (
          <Pressable
            onPress={() => setAll(true)}
            style={({ pressed }) => [styles.more, pressed && styles.morePressed]}
            accessibilityRole="button">
            <Text style={styles.moreText}>Show {chats.length - shown.length} more</Text>
          </Pressable>
        ) : null}
      </Rows>
      {note ? (
        <Text style={[styles.note, note.failed && styles.noteFailed]} accessibilityLiveRegion="polite">
          {note.text}
        </Text>
      ) : null}
    </>
  );
}

const RecentRow = memo(function RecentRow({
  chat,
  place,
  resuming,
  minute,
  onResume,
}: {
  chat: RecentInfo;
  place?: string;
  resuming: boolean;
  /** Ages the "5m" label once a minute. */
  minute: number;
  onResume: (chat: RecentInfo) => void;
}) {
  const styles = useStyles(makeStyles);
  const type = useType();
  return (
    <Row
      mark={<AgentIcon provider={chat.provider} size={20} />}
      title={chat.title}
      place={place}
      end={resuming ? <Working /> : <Text style={[type.meta, styles.age]}>{formatAge(Number(chat.activeAt), minute * 60_000)}</Text>}
      onPress={() => onResume(chat)}
    />
  );
});

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    age: { fontSize: 12, fontVariant: ['tabular-nums'] },
    more: { minHeight: 40, justifyContent: 'center', paddingLeft: 38, paddingRight: 8, borderRadius: 9 },
    morePressed: { backgroundColor: colors.active },
    moreText: { ...type.meta, fontSize: 13 },
    note: { ...type.meta, fontSize: 12.5, paddingHorizontal: 8, paddingTop: 8 },
    noteFailed: { color: colors.danger },
  });
};
