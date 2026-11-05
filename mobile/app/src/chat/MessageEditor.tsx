import { useMemo, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { useAgentNames } from '@/devices/hub';
import { fonts, keyboardFor, type Palette, useColors, useStyles } from '@/ui/theme';

/** The host's agent names, for the `@` chips a sent message wrote. */
export function useNamesIn(core: string): string[] {
  const names = useAgentNames(core);
  return useMemo(() => (names ? names.split('\n') : []), [names]);
}

/** Writing a sent message again where its bubble was. */
export type Editing = { onCancel: () => void; onSend: (text: string) => void };

export function MessageEditor({ text, editing }: { text: string; editing: Editing }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [draft, setDraft] = useState(text);
  const words = draft.trim();
  return (
    <View style={styles.editor}>
      <TextInput
        value={draft}
        onChangeText={setDraft}
        multiline
        autoFocus
        style={styles.editorText}
        selectionColor={colors.accent}
        keyboardAppearance={keyboardFor(colors)}
        accessibilityLabel="Edit message"
      />
      <View style={styles.editorActs}>
        <Pressable
          onPress={editing.onCancel}
          hitSlop={6}
          style={({ pressed }) => [styles.editorButton, pressed && { backgroundColor: colors.active }]}
          accessibilityRole="button">
          <Text style={styles.editorButtonText}>Cancel</Text>
        </Pressable>
        <Pressable
          onPress={() => editing.onSend(words)}
          disabled={!words}
          hitSlop={6}
          style={({ pressed }) => [styles.editorButton, styles.editorSend, (pressed || !words) && { opacity: 0.6 }]}
          accessibilityRole="button"
          accessibilityState={{ disabled: !words }}>
          <Text style={[styles.editorButtonText, styles.editorSendText]}>Send</Text>
        </Pressable>
      </View>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    editor: {
      width: '84%',
      gap: 8,
      paddingTop: 9,
      paddingBottom: 8,
      paddingHorizontal: 13,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.borderSelected,
      backgroundColor: colors.raised,
    },
    editorText: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 22, color: colors.ink, maxHeight: 264, padding: 0 },
    editorActs: { flexDirection: 'row', justifyContent: 'flex-end', gap: 6 },
    editorButton: {
      height: 30,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
    },
    editorButtonText: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.ink },
    editorSend: { backgroundColor: colors.ink, borderColor: colors.ink },
    editorSendText: { color: colors.sunken },
  });
};
