import { StyleSheet, Text, View } from 'react-native';

import type { ChatGoal } from '@shared/wire.generated';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { GoalSteps, StateMark } from './GoalLine';

export type Place = {
  host: string;
  project?: string;
  branch?: string;
  goal?: ChatGoal;
  /** The account the chat runs on, when its agent has more than one here. */
  account?: string;
};

/** Where the chat runs, quietly under the composer: its host, project and branch, then its goal and account at the right. */
export function ChatPlace({ place }: { place: Place }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const { host, project, branch, goal, account } = place;
  return (
    <View style={styles.line}>
      <View style={[styles.part, styles.shrinksMost]}>
        <Icon name="IconLaptop" size={12} color={colors.tertiary} />
        <Text style={styles.quiet} numberOfLines={1}>
          {host}
        </Text>
      </View>
      {project ? (
        <View style={[styles.part, styles.shrinks]}>
          <Icon name="IconFolder" size={12} color={colors.live} />
          <Text style={styles.project} numberOfLines={1}>
            {project}
          </Text>
        </View>
      ) : null}
      {branch ? (
        <View style={[styles.part, styles.shrinksMore]}>
          <Icon name="IconGit" size={11} color={colors.tertiary} />
          <Text style={styles.branch} numberOfLines={1}>
            {branch}
          </Text>
        </View>
      ) : null}
      <View style={styles.fill} />
      {goal ? (
        <View style={[styles.part, styles.shrinks]} accessibilityLabel={`Goal, ${goal.name}`}>
          <StateMark state={goal.state} />
          <Text style={styles.goal} numberOfLines={1}>
            {goal.name}
          </Text>
          <GoalSteps steps={goal.steps ?? []} />
        </View>
      ) : null}
      {account ? (
        <View style={[styles.part, styles.shrinks]}>
          <Icon name="IconUser" size={12} color={colors.tertiary} />
          <Text style={styles.quiet} numberOfLines={1}>
            {account}
          </Text>
        </View>
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    line: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 24, marginTop: 6, paddingHorizontal: 8 },
    part: { flexDirection: 'row', alignItems: 'center', gap: 6, minWidth: 0, paddingHorizontal: 4 },
    shrinks: { flexShrink: 1 },
    shrinksMore: { flexShrink: 2 },
    shrinksMost: { flexShrink: 4 },
    fill: { flexGrow: 1, minWidth: 4 },
    quiet: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    project: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.secondary },
    branch: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 10.5, color: colors.tertiary },
    goal: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
  });
};
