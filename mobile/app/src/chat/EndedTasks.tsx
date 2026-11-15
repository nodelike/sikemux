import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { EndedTask, EndedTasks } from '@shared/wire.generated';
import { endedDots, endedHow, endedName, sentence } from '@shared/chat/endedTasks';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

function stateInk(state: EndedTask['state'], colors: Palette): string {
  if (state === 'completed') return colors.live;
  if (state === 'failed') return colors.danger;
  return colors.inkFaint;
}

/** The background tasks that ended in one turn, as one line with a dot for each; a tap lists them in a sheet. */
export function EndedTasksLine({ tasks }: { tasks: EndedTasks }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [shown, setShown] = useState(false);
  const name = endedName(tasks, t);
  const how = endedHow(tasks, t);
  return (
    <>
      <Pressable
        onPress={() => {
          haptics.select();
          setShown(true);
        }}
        style={({ pressed }) => [styles.line, pressed && styles.pressed]}
        accessibilityRole="button"
        accessibilityLabel={`${name}, ${how}`}>
        <Icon name={tasks.failed ? 'IconCircleAlert' : 'IconCircleCheck'} size={15} color={tasks.failed ? colors.danger : colors.live} />
        <Text style={styles.name} numberOfLines={1}>
          {name}
        </Text>
        <View style={styles.dots}>
          {endedDots(tasks).map((state, at) => (
            <View key={at} style={[styles.dot, { backgroundColor: stateInk(state, colors) }, state === 'completed' && styles.doneDot]} />
          ))}
        </View>
        <Icon name="IconChevron" size={11} color={colors.tertiary} />
      </Pressable>
      <Sheet visible={shown} onClose={() => setShown(false)}>
        <Text style={styles.title}>{t('Background tasks')}</Text>
        <Text style={styles.how}>{how}</Text>
        {tasks.tasks.map((task, at) => (
          <EndedTaskRow key={at} task={task} first={at === 0} />
        ))}
      </Sheet>
    </>
  );
}

/** A task in the sheet; one that said how it went opens onto that. */
function EndedTaskRow({ task, first }: { task: EndedTask; first: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [open, setOpen] = useState(false);
  const stopped = task.state === 'stopped';
  const icon =
    task.state === 'completed' ? 'IconCheck' : task.state === 'failed' ? 'IconClose' : task.taskType === 'monitor' ? 'IconEye' : 'IconRun';
  const word = task.state === 'failed' ? t('failed') : stopped ? t('stopped') : null;
  const summary = task.summary?.trim();
  return (
    <Pressable
      onPress={summary ? () => setOpen(!open) : undefined}
      disabled={!summary}
      style={[styles.row, !first && styles.ruled]}
      accessibilityRole={summary ? 'button' : undefined}
      accessibilityState={summary ? { expanded: open } : undefined}>
      <View style={styles.rowHead}>
        <Icon name={icon} size={16} color={stopped ? colors.inkFaint : stateInk(task.state, colors)} />
        <View style={styles.rowName}>
          <Text style={[styles.rowText, stopped && styles.rowStopped]} numberOfLines={1}>
            {sentence(task.name)}
          </Text>
          {task.times > 1 ? <Text style={styles.times}>×{task.times}</Text> : null}
        </View>
        {word ? <Text style={[styles.word, task.state === 'failed' && { color: colors.danger }]}>{word}</Text> : null}
        {summary ? (
          <View style={open && styles.turned}>
            <Icon name="IconChevron" size={12} color={colors.inkFaint} />
          </View>
        ) : null}
      </View>
      {summary && open ? (
        <Text style={styles.summary} selectable>
          {summary}
        </Text>
      ) : null}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    line: {
      flexDirection: 'row',
      alignItems: 'center',
      alignSelf: 'flex-start',
      gap: 8,
      height: 32,
      marginLeft: -6,
      marginVertical: 2,
      paddingHorizontal: 6,
      borderRadius: 8,
    },
    pressed: { backgroundColor: colors.active },
    name: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    dots: { flexDirection: 'row', gap: 3 },
    dot: { width: 6, height: 6, borderRadius: 3 },
    doneDot: { opacity: 0.85 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 17, color: colors.ink, paddingHorizontal: 4 },
    how: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingHorizontal: 4, paddingTop: 4, paddingBottom: 10 },
    row: { paddingHorizontal: 4 },
    ruled: { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: colors.border },
    rowHead: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 46 },
    rowName: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 6 },
    rowText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 15, color: colors.secondary },
    rowStopped: { color: colors.tertiary },
    times: {
      fontFamily: fonts.mono,
      fontSize: 12,
      color: colors.tertiary,
      paddingHorizontal: 4,
      borderWidth: 1,
      borderColor: colors.border,
      borderRadius: 4,
    },
    word: { fontFamily: fonts.mono, fontSize: 13, color: colors.tertiary },
    turned: { transform: [{ rotate: '90deg' }] },
    summary: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.inkDim, paddingLeft: 28, paddingBottom: 10 },
  });
