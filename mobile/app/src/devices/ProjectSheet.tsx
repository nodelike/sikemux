import { useState, type ReactNode } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import type { ProjectInfo } from '@/core/protocol';
import { DrawnIcon, Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

function home(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

type Props = {
  visible: boolean;
  onClose: () => void;
  /** The Mac the projects are open on, named beside the title. */
  device?: string;
  projects: ProjectInfo[];
  /** The chosen project's id, or null for every project. */
  chosen: string | null;
  onChoose: (project: string | null) => void;
  /** Offers every project at once, described by this line. */
  all?: string;
  /** What runs in a project, shown at the end of its row. */
  tail?: (project: ProjectInfo) => ReactNode;
};

/** The Mac's project switcher: the projects open in its app, searched by name or path. */
export function ProjectSheet({ visible, onClose, device, projects, chosen, onChoose, all, tail }: Props) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const [query, setQuery] = useState('');
  const needle = query.trim().toLowerCase();
  const found = projects.filter((project) => `${project.name} ${project.path}`.toLowerCase().includes(needle));
  const showAll = all !== undefined && !needle;
  const choose = (project: string | null) => {
    setQuery('');
    onChoose(project);
  };
  return (
    <Sheet visible={visible} onClose={onClose} tall={projects.length > 6}>
      <View style={styles.head}>
        <Text style={styles.title}>Projects</Text>
        {device ? <Text style={type.meta}>{device}</Text> : null}
      </View>
      <View style={[styles.search, query ? { borderColor: colors.borderSelected } : null]}>
        <Icon name="IconSearch" size={16} color={colors.tertiary} />
        <TextInput
          value={query}
          onChangeText={setQuery}
          placeholder="Search projects…"
          placeholderTextColor={colors.tertiary}
          style={styles.searchInput}
          autoCorrect={false}
          autoCapitalize="none"
          keyboardAppearance="dark"
          returnKeyType="go"
          onSubmitEditing={() => found[0] && choose(found[0].id)}
        />
      </View>
      <ScrollView keyboardShouldPersistTaps="handled" style={{ marginTop: 12 }}>
        <View style={styles.group}>
          {showAll ? (
            <Pressable onPress={() => choose(null)} style={[styles.row, chosen === null && styles.rowOn]} accessibilityRole="button">
              <DrawnIcon name="folders" size={16} color={colors.live} />
              <View style={{ flex: 1 }}>
                <Text style={styles.name}>All projects</Text>
                <Text style={styles.detail}>{all}</Text>
              </View>
              {chosen === null ? <Icon name="IconCheck" size={16} color={colors.ink} /> : null}
            </Pressable>
          ) : null}
          {found.map((project, index) => {
            const on = project.id === chosen;
            return (
              <Pressable
                key={project.id}
                onPress={() => choose(project.id)}
                style={[styles.row, on && styles.rowOn, (showAll || index > 0) && styles.divided]}
                accessibilityRole="button">
                <Icon name="IconFolder" size={16} color={colors.live} />
                <View style={{ flex: 1 }}>
                  <Text style={styles.name}>{project.name}</Text>
                  <Text style={styles.path} numberOfLines={1} ellipsizeMode="head">
                    {home(project.path)}
                  </Text>
                </View>
                {tail?.(project)}
                {on ? <Icon name="IconCheck" size={16} color={colors.ink} /> : null}
              </Pressable>
            );
          })}
        </View>
        {!found.length ? <Text style={[type.meta, { textAlign: 'center', paddingTop: 24 }]}>No open project matches.</Text> : null}
      </ScrollView>
    </Sheet>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    head: {
      flexDirection: 'row',
      alignItems: 'baseline',
      justifyContent: 'space-between',
      paddingHorizontal: 4,
      paddingTop: 2,
      paddingBottom: 12,
    },
    title: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    search: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      height: 44,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    searchInput: { flex: 1, fontFamily: fonts.ui, fontSize: 15.5, color: colors.ink },
    group: { borderRadius: 14, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    row: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 56, paddingHorizontal: 14, paddingVertical: 8 },
    rowOn: { backgroundColor: colors.active },
    divided: { borderTopWidth: 1, borderTopColor: colors.border },
    name: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.ink },
    detail: { ...type.meta, marginTop: 1 },
    path: { fontFamily: fonts.mono, fontSize: 11.5, color: colors.tertiary, marginTop: 1 },
  });
};
