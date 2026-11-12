import { StyleSheet, Text, View } from 'react-native';

import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

export type Place = {
  /** The host, when the account has more than one to tell apart. */
  host?: string;
  project?: string;
  branch?: string;
  /** The account the chat runs on, when its agent has more than one here. */
  account?: string;
};

/**
 * Where the chat runs, quietly under the composer as the Mac's line under the box: the host when there
 * are several, its project and branch with a slash between, and its account as its mark at the right, so the
 * branch keeps its whole name.
 */
export function ChatPlace({ place }: { place: Place }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const { host, project, branch, account } = place;
  return (
    <View style={styles.line}>
      {host ? (
        <View style={[styles.part, styles.shrinksMost]}>
          <Icon name="IconLaptop" size={12} color={colors.tertiary} />
          <Text style={styles.quiet} numberOfLines={1}>
            {host}
          </Text>
        </View>
      ) : null}
      {host && (project || branch) ? <Slash /> : null}
      {project ? (
        <View style={styles.part}>
          <Icon name="IconFolder" size={12} color={colors.live} />
          <Text style={styles.project} numberOfLines={1}>
            {project}
          </Text>
        </View>
      ) : null}
      {project && branch ? <Slash /> : null}
      {branch ? (
        <View style={[styles.part, styles.shrinks]}>
          <Icon name="IconGit" size={11} color={colors.tertiary} />
          <Text style={styles.branch} numberOfLines={1}>
            {branch}
          </Text>
        </View>
      ) : null}
      <View style={styles.fill} />
      {account ? (
        <View style={styles.part} accessible accessibilityLabel={`Account, ${account}`}>
          <Icon name="IconUser" size={13} color={colors.tertiary} />
        </View>
      ) : null}
    </View>
  );
}

function Slash() {
  const styles = useStyles(makeStyles);
  return (
    <Text style={styles.slash} importantForAccessibility="no" accessibilityElementsHidden>
      /
    </Text>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    line: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 24, marginTop: 6, paddingHorizontal: 8 },
    part: { flexDirection: 'row', alignItems: 'center', gap: 6, minWidth: 0, paddingHorizontal: 4 },
    shrinks: { flexShrink: 1 },
    shrinksMost: { flexShrink: 4 },
    fill: { flexGrow: 1, minWidth: 4 },
    quiet: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    project: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.secondary },
    branch: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 10.5, color: colors.tertiary },
    slash: { fontFamily: fonts.ui, fontSize: 13, color: colors.borderStrong },
  });
};
