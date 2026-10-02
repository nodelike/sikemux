import { useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import * as Haptics from 'expo-haptics';

import { pickerSlots, sessionConfigs, type SessionConfig } from '@mac/chat/sessionConfig';
import { toolKind, toolTarget } from '@mac/chat/toolLabels';
import type { AcpPermissionRequest, ChatState } from '@mac/chat/types';
import { providerName } from '@/devices/words';
import { AgentIcon, Icon, isProvider } from '@/ui/Icon';
import { Track, useKeyboardShown } from '@/ui/parts';
import { Sheet } from '@/ui/Sheet';
import { ComposerInput } from './ComposerInput';
import { brand, fonts, type Palette, useColors, useStyles } from '@/ui/theme';

function current(config?: SessionConfig): string | undefined {
  if (!config) return undefined;
  return config.options.find((option) => option.value === config.currentValue)?.label ?? config.currentValue;
}

function askTitle(request: AcpPermissionRequest): string {
  const kind = toolKind(request.toolCall);
  if (kind === 'run') return 'Run a command?';
  if (kind === 'edit' || kind === 'move' || kind === 'delete') return 'Change a file?';
  if (kind === 'fetch') return 'Open a web page?';
  return 'Allow this?';
}

/** Docked on the composer until it is answered: the Mac's permission card, with the agent's own options. */
function PermissionDock({
  request,
  provider,
  onAnswer,
}: {
  request: AcpPermissionRequest;
  provider: string;
  onAnswer: (optionId: string | null) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const rejects = request.options.filter((option) => option.kind.startsWith('reject'));
  const always = request.options.filter((option) => option.kind === 'allow_always');
  const once = request.options.filter((option) => option.kind === 'allow_once');
  const ordered = [...rejects, ...always, ...once];
  const primary = once[0] ?? always[0];
  return (
    <View style={styles.dock}>
      <View style={styles.dockHead}>
        <Icon name="IconShieldBolt" size={16} color={colors.ink} />
        <View style={{ flex: 1 }}>
          <Text style={styles.dockTitle}>{askTitle(request)}</Text>
          <Text style={styles.dockDetail}>{providerName(provider)} needs permission to continue</Text>
        </View>
      </View>
      <View style={styles.dockCmd}>
        <Text style={styles.dockCmdText} numberOfLines={3}>
          {toolKind(request.toolCall) === 'run' ? <Text style={{ color: colors.toolRun }}>$ </Text> : null}
          {toolTarget(request.toolCall)}
        </Text>
      </View>
      <View style={styles.dockActs}>
        {(ordered.length ? ordered : [{ optionId: '', name: 'Cancel', kind: 'reject_once' }]).map((option) => {
          const go = option === primary;
          return (
            <Pressable
              key={option.optionId || option.name}
              onPress={() => {
                Haptics.selectionAsync();
                onAnswer(option.optionId || null);
              }}
              style={({ pressed }) => [styles.act, go && styles.actGo, pressed && { opacity: 0.8 }]}
              accessibilityRole="button">
              <Text
                style={[styles.actText, go && styles.actGoText, option.kind.startsWith('reject') && { color: colors.secondary }]}
                numberOfLines={1}>
                {option.name}
              </Text>
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

function ConfigSheet({
  visible,
  onClose,
  provider,
  configs,
  usage,
  onPick,
}: {
  visible: boolean;
  onClose: () => void;
  provider: string;
  configs: SessionConfig[];
  usage: ChatState['usage'];
  onPick: (config: SessionConfig, value: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const slots = pickerSlots(configs, provider as never);
  const model = slots[0]?.config;
  const effort = slots[1]?.config;
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheetHead}>
        <AgentIcon provider={provider} size={20} />
        <Text style={styles.sheetTitle}>{providerName(provider)}</Text>
      </View>
      {model ? (
        <>
          <Text style={styles.sheetLabel}>Model</Text>
          <View style={styles.group}>
            {model.options.map((option, index) => {
              const on = option.value === model.currentValue;
              return (
                <Pressable
                  key={option.value}
                  onPress={() => onPick(model, option.value)}
                  style={[styles.option, on && { backgroundColor: colors.active }, index > 0 && styles.optionDivided]}>
                  <AgentIcon provider={provider} size={18} />
                  <Text style={[styles.optionText, on && { color: colors.ink }]} numberOfLines={1}>
                    {option.label}
                  </Text>
                  {on ? <Icon name="IconCheck" size={17} color={colors.ink} /> : null}
                </Pressable>
              );
            })}
          </View>
        </>
      ) : null}
      {effort ? (
        <>
          <Text style={styles.sheetLabel}>Effort</Text>
          <Track
            value={effort.currentValue}
            onChange={(value) => onPick(effort, value)}
            options={effort.options.map((option) => ({ value: option.value, label: option.label }))}
          />
        </>
      ) : null}
      {usage ? (
        <View style={styles.context}>
          <View style={styles.contextTop}>
            <Text style={styles.contextLabel}>Context</Text>
            <Text style={styles.contextValue}>
              {Math.round(usage.used / 1000)}k of {Math.round(usage.size / 1000)}k
            </Text>
          </View>
          <View style={styles.contextBar}>
            <View
              style={[
                styles.contextFill,
                {
                  width: `${Math.min(100, (usage.used / usage.size) * 100)}%`,
                  backgroundColor: isProvider(provider) ? brand[provider] : colors.accent,
                },
              ]}
            />
          </View>
        </View>
      ) : null}
    </Sheet>
  );
}

export function Composer({
  state,
  provider,
  placeholder,
  permissionMode,
  watchOnly,
  offline,
  onSend,
  onStop,
  onAnswer,
  onConfig,
}: {
  state: ChatState;
  provider: string;
  placeholder: string;
  permissionMode: string;
  watchOnly: boolean;
  /** The Mac is out of reach, so a message is kept until it is back. */
  offline: boolean;
  onSend: (text: string) => void;
  onStop: () => void;
  onAnswer: (requestId: string, optionId: string | null) => void;
  onConfig: (configId: string, value: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [draft, setDraft] = useState('');
  const [focused, setFocused] = useState(false);
  const input = useRef<TextInput>(null);
  const [sheet, setSheet] = useState(false);
  const typing = useKeyboardShown();
  const configs = sessionConfigs(state.setup);
  const slots = pickerSlots(configs, provider as never);
  const model = current(slots[0]?.config);
  const effort = current(slots[1]?.config);
  const request = state.permissions[0];
  const yolo = permissionMode === 'bypass' || permissionMode === 'bypassPermissions' || permissionMode === 'full-access';

  if (watchOnly) {
    return (
      <SafeAreaView edges={typing ? [] : ['bottom']} style={styles.wrap}>
        <View style={[styles.composer, styles.watch]}>
          <Icon name="IconEye" size={17} color={colors.inkDim} />
          <Text style={styles.watchText}>Watching. This phone can answer permission requests; the Mac can give it full access.</Text>
        </View>
      </SafeAreaView>
    );
  }

  const send = () => {
    const text = draft.trim();
    if (!text || offline) return;
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    onSend(text);
    // Clearing the state alone leaves text the keyboard is still composing.
    input.current?.clear();
    setDraft('');
  };

  return (
    <SafeAreaView edges={typing ? [] : ['bottom']} style={styles.wrap}>
      {request ? <PermissionDock request={request} provider={provider} onAnswer={(option) => onAnswer(request.requestId, option)} /> : null}
      <View style={[styles.composer, focused && { borderColor: colors.borderSelected }]}>
        <ComposerInput
          ref={input}
          value={draft}
          onChangeText={setDraft}
          placeholder={placeholder}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        <View style={styles.bar}>
          <View style={styles.yolo}>
            <Icon name={yolo ? 'IconShieldBolt' : 'IconShield'} size={13} color={yolo ? colors.accent : colors.inkFaint} />
            <Text style={[styles.yoloText, yolo && { color: colors.accent }]}>{yolo ? 'yolo' : 'safe'}</Text>
          </View>
          {model ? (
            <Pressable
              style={styles.picker}
              onPress={() => setSheet(true)}
              accessibilityRole="button"
              accessibilityLabel="Model and effort">
              <AgentIcon provider={provider} size={16} />
              <Text style={[styles.pickerText, { color: colors.accent }]} numberOfLines={1}>
                {model}
              </Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.accent} />
              </View>
            </Pressable>
          ) : null}
          {effort ? (
            <Pressable style={styles.picker} onPress={() => setSheet(true)} accessibilityRole="button">
              <Text style={styles.pickerText}>{effort}</Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.inkDim} />
              </View>
            </Pressable>
          ) : null}
          <View style={{ flex: 1 }} />
          {draft.trim() || !state.running ? (
            <Pressable
              onPress={send}
              style={[styles.send, (!draft.trim() || offline) && { opacity: 0.28 }]}
              disabled={offline}
              accessibilityRole="button"
              accessibilityLabel="Send"
              accessibilityState={{ disabled: offline || !draft.trim() }}>
              <Icon name="IconArrowUp" size={16} color={colors.ground} />
            </Pressable>
          ) : (
            <Pressable onPress={onStop} style={styles.send} accessibilityRole="button" accessibilityLabel="Stop">
              <View style={styles.stop} />
            </Pressable>
          )}
        </View>
      </View>
      <ConfigSheet
        visible={sheet}
        onClose={() => setSheet(false)}
        provider={provider}
        configs={configs}
        usage={state.usage}
        onPick={(config, value) => {
          onConfig(config.id, value);
          setSheet(false);
        }}
      />
    </SafeAreaView>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    composer: { padding: 6, borderRadius: 16, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.composer },
    bar: { flexDirection: 'row', alignItems: 'center', gap: 2, paddingTop: 6 },
    yolo: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingHorizontal: 8 },
    yoloText: { fontFamily: fonts.uiSemibold, fontSize: 11, letterSpacing: 0.9, textTransform: 'uppercase', color: colors.inkFaint },
    picker: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 34, paddingHorizontal: 7, borderRadius: 7, maxWidth: 160 },
    pickerText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    send: { width: 34, height: 34, borderRadius: 17, backgroundColor: colors.ink, alignItems: 'center', justifyContent: 'center' },
    stop: { width: 10, height: 10, borderRadius: 2, backgroundColor: colors.ground },
    watch: { flexDirection: 'row', alignItems: 'center', gap: 10, padding: 14 },
    watchText: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.inkDim },

    dock: {
      marginHorizontal: 10,
      marginBottom: -1,
      padding: 12,
      borderWidth: 1,
      borderBottomWidth: 0,
      borderColor: colors.borderStrong,
      borderTopLeftRadius: 16,
      borderTopRightRadius: 16,
      backgroundColor: colors.overlay,
    },
    dockHead: { flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
    dockTitle: { fontFamily: fonts.uiSemibold, fontSize: 15, letterSpacing: -0.2, color: colors.ink },
    dockDetail: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, marginTop: 1 },
    dockCmd: {
      marginTop: 10,
      paddingVertical: 9,
      paddingHorizontal: 11,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    dockCmdText: { fontFamily: fonts.mono, fontSize: 12.5, lineHeight: 18, color: colors.ink },
    dockActs: { flexDirection: 'row', gap: 6, marginTop: 10 },
    act: {
      flex: 1,
      minHeight: 38,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
      paddingHorizontal: 6,
    },
    actGo: { backgroundColor: colors.ink, borderColor: colors.ink },
    actText: { fontFamily: fonts.uiMedium, fontSize: 14, color: colors.ink },
    actGoText: { fontFamily: fonts.uiSemibold, color: colors.ground },

    sheetHead: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 4 },
    sheetTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    sheetLabel: {
      fontFamily: fonts.uiSemibold,
      fontSize: 13,
      color: colors.tertiary,
      paddingTop: 18,
      paddingBottom: 8,
      paddingHorizontal: 6,
    },
    group: { borderRadius: 14, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 52, paddingHorizontal: 14 },
    optionDivided: { borderTopWidth: 1, borderTopColor: colors.border },
    optionText: { flex: 1, fontFamily: fonts.uiMedium, fontSize: 15.5, color: colors.secondary },
    context: { marginTop: 20, paddingHorizontal: 4 },
    contextTop: { flexDirection: 'row', justifyContent: 'space-between' },
    contextLabel: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary },
    contextValue: { fontFamily: fonts.mono, fontSize: 12, color: colors.tertiary },
    contextBar: { marginTop: 8, height: 4, borderRadius: 2, backgroundColor: colors.border, overflow: 'hidden' },
    contextFill: { height: 4, borderRadius: 2 },
  });
};
