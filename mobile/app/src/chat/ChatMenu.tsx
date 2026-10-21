import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import * as Linking from 'expo-linking';

import { Button } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

function Item({ icon, label, danger, onPress }: { icon: IconName; label: string; danger?: boolean; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const color = danger ? colors.danger : colors.ink;
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.item, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button">
      <Icon name={icon} size={18} color={color} />
      <Text style={[styles.itemText, { color }]}>{label}</Text>
    </Pressable>
  );
}

/** The chat's ⋯ menu: what was sent before, a link that opens this chat on a phone, and stopping the agent. */
export function ChatMenu({
  visible,
  onClose,
  core,
  agentId,
  agentName,
  hostName,
  onRecent,
  onStop,
}: {
  visible: boolean;
  onClose: () => void;
  core: string;
  agentId: string;
  agentName: string;
  hostName: string;
  /** Absent while nothing was sent. */
  onRecent?: () => void;
  /** Absent for a phone that can only watch. */
  onStop?: () => Promise<boolean>;
}) {
  const styles = useStyles(makeStyles);
  const [confirming, setConfirming] = useState(false);
  const [stopping, setStopping] = useState(false);
  const close = () => {
    setConfirming(false);
    onClose();
  };
  return (
    <Sheet visible={visible} onClose={close}>
      {confirming && onStop ? (
        <View style={styles.confirm}>
          <Text style={styles.title}>Stop this chat?</Text>
          <Text style={styles.body}>
            {agentName} stops and closes on {hostName}, along with anything it still has running.
          </Text>
          <Button
            kind="danger"
            title={stopping ? 'Stopping…' : 'Stop chat'}
            disabled={stopping}
            onPress={() => {
              setStopping(true);
              void onStop().finally(() => {
                setStopping(false);
                close();
              });
            }}
          />
          <Button title="Cancel" onPress={() => setConfirming(false)} />
        </View>
      ) : (
        <View style={styles.items}>
          {onRecent ? (
            <Item
              icon="IconClock"
              label="Recent prompts"
              onPress={() => {
                close();
                onRecent();
              }}
            />
          ) : null}
          <Item
            icon="IconCopy"
            label="Copy link"
            onPress={() => {
              const link = Linking.createURL(`device/${encodeURIComponent(core)}/chat/${encodeURIComponent(agentId)}`);
              void Clipboard.setStringAsync(link).then(() => haptics.select());
              close();
            }}
          />
          {onStop ? <Item icon="IconStop" label="Stop chat" danger onPress={() => setConfirming(true)} /> : null}
        </View>
      )}
    </Sheet>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    items: { gap: 8, paddingBottom: 4 },
    item: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 52,
      paddingHorizontal: 14,
      borderRadius: 14,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    itemText: { fontFamily: fonts.uiMedium, fontSize: 15.5 },
    confirm: { gap: 10, paddingBottom: 4 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink, paddingHorizontal: 4 },
    body: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 20, color: colors.tertiary, paddingHorizontal: 4, paddingBottom: 6 },
  });
};
