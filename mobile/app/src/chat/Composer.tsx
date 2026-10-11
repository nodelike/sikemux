import { memo, useMemo, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, PanResponder, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Circle, Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { toolDiff } from '@mac/chat/diff';
import { pickerSlots, sessionConfigs, type SessionConfig } from '@mac/chat/sessionConfig';
import { toolKind, toolLabel, toolPath } from '@mac/chat/toolLabels';
import type { AcpAvailableCommand, AcpPermissionRequest, AcpToolCall, ChatState, ContextUsage } from '@mac/chat/types';
import { providerName } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Track } from '@/ui/controls';
import { useKeyboardShown } from '@/ui/screen';
import { Sheet } from '@/ui/Sheet';
import { brand, fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { ComposerAttachments } from './Attachments';
import { ComposerInput } from './ComposerInput';
import { pickFiles, pickPhotos } from './pick';
import { MAX_ATTACHMENTS, type Attachment, type ChatSession } from './session';
import { useDraft } from './useChat';
import { matchingCommands } from './commands';

function current(config?: SessionConfig): string | undefined {
  if (!config) return undefined;
  return config.options.find((option) => option.value === config.currentValue)?.label ?? config.currentValue;
}

export function askTitle(request: AcpPermissionRequest): string {
  const kind = toolKind(request.toolCall);
  if (kind === 'run') return 'Run a command?';
  if (kind === 'edit' || kind === 'move' || kind === 'delete') return 'Change a file?';
  if (kind === 'fetch') return 'Open a web page?';
  return 'Allow this?';
}

/** All of what the agent asks to run or touch, since the end of a command can change what it does. */
function asked(tool: AcpToolCall): string {
  const command = (tool.rawInput as { command?: unknown } | undefined)?.command;
  if (typeof command === 'string' && command.trim()) return command.trim();
  if (Array.isArray(command) && command.every((word) => typeof word === 'string')) return command.join(' ');
  return toolPath(tool) ?? toolLabel(tool.title).name.trim();
}

/**
 * Sits above the composer until it is answered: the Mac's permission card, with the agent's own
 * options, and Cancel when none of them says no.
 */
function PermissionDock({
  requests,
  provider,
  answering,
  onAnswer,
}: {
  requests: AcpPermissionRequest[];
  provider: string;
  answering: ReadonlySet<string>;
  onAnswer: (requestId: string, optionId: string | null) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [page, setPage] = useState(0);
  const index = Math.min(page, requests.length - 1);
  const request = requests[index];
  const busy = answering.has(request.requestId);
  const rejects = request.options.filter((option) => option.kind.startsWith('reject'));
  const always = request.options.filter((option) => option.kind === 'allow_always');
  const once = request.options.filter((option) => option.kind === 'allow_once');
  const cancel = rejects.length ? [] : [{ optionId: '', name: 'Cancel', kind: 'reject_once' }];
  const ordered = [...cancel, ...rejects, ...always, ...once];
  const primary = once[0] ?? always[0];
  const run = toolKind(request.toolCall) === 'run';
  const diff = toolDiff(request.toolCall);
  const title = askTitle(request);
  return (
    <View
      style={styles.dock}
      accessibilityRole="alert"
      accessibilityLabel={`${title} ${providerName(provider)} needs permission to continue`}>
      <View style={styles.dockHead}>
        <Icon name="IconShieldBolt" size={16} color={colors.ink} />
        <View style={{ flex: 1 }}>
          <Text style={styles.dockTitle}>{title}</Text>
          <Text style={styles.dockDetail}>{providerName(provider)} needs permission to continue</Text>
        </View>
        {requests.length > 1 ? (
          <View style={styles.pages}>
            <Pressable
              onPress={() => setPage(index - 1)}
              disabled={index === 0}
              hitSlop={8}
              style={[styles.pageButton, index === 0 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel="Previous request">
              <View style={{ transform: [{ rotate: '180deg' }] }}>
                <Icon name="IconChevron" size={12} color={colors.secondary} />
              </View>
            </Pressable>
            <Text style={styles.pageText} accessibilityLabel={`Request ${index + 1} of ${requests.length}`}>
              {index + 1} of {requests.length}
            </Text>
            <Pressable
              onPress={() => setPage(index + 1)}
              disabled={index === requests.length - 1}
              hitSlop={8}
              style={[styles.pageButton, index === requests.length - 1 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel="Next request">
              <Icon name="IconChevron" size={12} color={colors.secondary} />
            </Pressable>
          </View>
        ) : null}
      </View>
      <ScrollView style={styles.dockCmd} contentContainerStyle={styles.dockCmdPad} nestedScrollEnabled>
        <Text style={styles.dockCmdText} selectable>
          {run ? <Text style={{ color: colors.toolRun }}>$ </Text> : null}
          {asked(request.toolCall)}
        </Text>
        {diff ? (
          <Text style={[styles.dockCmdText, { marginTop: 4 }]}>
            <Text style={{ color: colors.gitAdded }}>+{diff.adds}</Text> <Text style={{ color: colors.gitDeleted }}>−{diff.dels}</Text>
          </Text>
        ) : null}
      </ScrollView>
      <View style={styles.dockActs}>
        {ordered.map((option) => {
          const go = option === primary;
          return (
            <Pressable
              key={option.optionId || option.name}
              disabled={busy}
              onPress={() => {
                haptics.select();
                onAnswer(request.requestId, option.optionId || null);
              }}
              style={({ pressed }) => [styles.act, go && styles.actGo, pressed && { opacity: 0.8 }, busy && { opacity: 0.5 }]}
              accessibilityRole="button"
              accessibilityState={{ disabled: busy, busy }}>
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

function Commands({ matches, onPick }: { matches: AcpAvailableCommand[]; onPick: (name: string) => void }) {
  const styles = useStyles(makeStyles);
  if (!matches.length) return null;
  return (
    <View style={styles.commands} accessibilityRole="menu">
      {matches.map((command) => (
        <Pressable
          key={command.name}
          onPress={() => onPick(command.name)}
          style={({ pressed }) => [styles.command, pressed && { opacity: 0.8 }]}
          accessibilityRole="menuitem"
          accessibilityLabel={`/${command.name}, ${command.description}`}>
          <Text style={styles.commandName}>/{command.name}</Text>
          <Text style={styles.commandText} numberOfLines={1}>
            {command.description}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

const tokens = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 });

/** An adapter names its own currency, and Intl throws on a code it does not know. */
function money({ amount, currency }: NonNullable<ContextUsage['cost']>): string {
  try {
    return new Intl.NumberFormat('en', { style: 'currency', currency, maximumFractionDigits: 2 }).format(amount);
  } catch {
    return `${amount.toFixed(2)} ${currency}`;
  }
}

function usedShare(usage: ContextUsage): number {
  return Math.min(1, Math.max(0, usage.used / usage.size));
}

/** The Mac's colour for how full the context is: the agent's own, then warm, then hot. */
function usageColor(usage: ContextUsage, provider: string, colors: Palette): string {
  const share = usedShare(usage);
  if (share >= 0.9) return colors.danger;
  if (share >= 0.7) return colors.warn;
  return provider === 'claude' || provider === 'codex' ? brand[provider] : colors.accent;
}

const RING = 14;
const RING_STROKE = 1.8;

/** How much of the context window the chat has used, as the Mac's composer rings it. A tap opens the settings sheet. */
function ContextRing({ usage, provider, onPress }: { usage: ContextUsage; provider: string; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const radius = (RING - RING_STROKE) / 2;
  const around = 2 * Math.PI * radius;
  const share = usedShare(usage);
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.ring, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={`Context window ${Math.round(share * 100)}% used`}>
      <Svg width={RING} height={RING} style={{ transform: [{ rotate: '-90deg' }] }}>
        <Circle cx={RING / 2} cy={RING / 2} r={radius} stroke={colors.border} strokeWidth={RING_STROKE} fill="none" />
        <Circle
          cx={RING / 2}
          cy={RING / 2}
          r={radius}
          stroke={usageColor(usage, provider, colors)}
          strokeWidth={RING_STROKE}
          strokeDasharray={`${share * around} ${around}`}
          strokeLinecap="round"
          fill="none"
        />
      </Svg>
    </Pressable>
  );
}

/** Agents whose host can switch between asking first and running without asking mid-chat. */
const SWITCHES_MODE = new Set(['claude', 'codex', 'hermes']);

export function runsWithoutAsking(mode: string): boolean {
  return mode === 'bypass' || mode === 'bypassPermissions' || mode === 'full-access';
}

/** YOLO or safe, as the Mac's toggle says it; a tap switches it where the host can. */
/** The Mac's YOLO word, in its rainbow. */
function YoloWord() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const stops = [colors.accent, colors.cmd, colors.danger, colors.warn, colors.live, colors.accent];
  return (
    <MaskedView maskElement={<Text style={styles.yoloText}>yolo</Text>}>
      <Text style={[styles.yoloText, { opacity: 0 }]} importantForAccessibility="no" accessibilityElementsHidden>
        yolo
      </Text>
      <Svg style={StyleSheet.absoluteFill}>
        <Defs>
          {/* Twice the word's width, as the Mac draws it, so the word shows the first half. */}
          <LinearGradient id="yolo" x1="0" y1="0" x2="2" y2="0">
            {stops.map((color, index) => (
              <Stop key={index} offset={index / (stops.length - 1)} stopColor={color} />
            ))}
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#yolo)" />
      </Svg>
    </MaskedView>
  );
}

export function YoloToggle({
  mode,
  provider,
  locked,
  onToggle,
}: {
  mode: string;
  provider: string;
  /** A turn is running or something waits on an answer, and the host refuses a switch then. */
  locked: boolean;
  onToggle: (mode: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const on = runsWithoutAsking(mode);
  const switches = SWITCHES_MODE.has(provider);
  const look = (
    <>
      <Icon name={on ? 'IconShieldBolt' : 'IconShield'} size={13} color={on ? colors.accent : colors.inkFaint} />
      {on ? <YoloWord /> : <Text style={styles.yoloText}>safe</Text>}
    </>
  );
  if (!switches) {
    return (
      <View style={styles.yolo} accessible accessibilityLabel={on ? 'Runs without asking' : 'Asks before acting'}>
        {look}
      </View>
    );
  }
  return (
    <Pressable
      onPress={() => {
        haptics.select();
        onToggle(on ? 'workspace-write' : 'bypass');
      }}
      disabled={locked}
      style={({ pressed }) => [styles.yolo, pressed && { backgroundColor: colors.active }, locked && { opacity: 0.55 }]}
      accessibilityRole="switch"
      accessibilityLabel="Run without asking"
      accessibilityState={{ checked: on, disabled: locked }}
      accessibilityHint={locked ? 'Changes once the turn ends' : undefined}>
      {look}
    </Pressable>
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
              {Math.round(usedShare(usage) * 100)}% · {tokens.format(usage.used)} of {tokens.format(usage.size)}
            </Text>
          </View>
          <View style={styles.contextBar}>
            <View
              style={[styles.contextFill, { width: `${usedShare(usage) * 100}%`, backgroundColor: usageColor(usage, provider, colors) }]}
            />
          </View>
          {usage.cost ? <Text style={styles.cost}>Session cost {money(usage.cost)}</Text> : null}
        </View>
      ) : null}
    </Sheet>
  );
}

export type Source = 'photos' | 'files';

/** Where a message's photos and files come from. */
export function AttachSheet({
  visible,
  onClose,
  onPick,
  onDismiss,
}: {
  visible: boolean;
  onClose: () => void;
  onPick: (source: Source) => void;
  onDismiss: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const sources: { source: Source; label: string; icon: 'IconImage' | 'IconFile' }[] = [
    { source: 'photos', label: 'Photos', icon: 'IconImage' },
    { source: 'files', label: 'Files', icon: 'IconFile' },
  ];
  return (
    <Sheet visible={visible} onClose={onClose} onDismiss={onDismiss}>
      <View style={styles.sources}>
        {sources.map(({ source, label, icon }) => (
          <Pressable
            key={source}
            onPress={() => onPick(source)}
            style={({ pressed }) => [styles.source, pressed && { backgroundColor: colors.active }]}
            accessibilityRole="button">
            <Icon name={icon} size={19} color={colors.ink} />
            <Text style={styles.sourceText}>{label}</Text>
          </Pressable>
        ))}
      </View>
    </Sheet>
  );
}

/** How far up an empty composer is pulled before it opens the recent prompts. */
const PULL_UP = 18;

export const Composer = memo(function Composer({
  session,
  provider,
  running,
  setup,
  usage,
  commands,
  requests,
  answering,
  placeholder,
  permissionMode,
  modeLocked,
  watchOnly,
  offline,
  hostName,
  onSent,
  attachments,
  strip,
  onRecent,
}: {
  session: ChatSession;
  provider: string;
  running: boolean;
  setup: ChatState['setup'];
  usage: ChatState['usage'];
  commands: AcpAvailableCommand[];
  /** The permission requests waiting on an answer, the first one shown first. */
  requests: AcpPermissionRequest[];
  answering: ReadonlySet<string>;
  placeholder: string;
  permissionMode: string;
  /** The host would refuse a switch between YOLO and safe now. */
  modeLocked: boolean;
  watchOnly: boolean;
  /** The host is out of reach, so a message is kept until it is back. */
  offline: boolean;
  hostName: string;
  onSent: () => void;
  /** Picked for the next message. */
  attachments: readonly Attachment[];
  /** What is still going, docked on the composer's top edge while nothing more urgent is. */
  strip: ReactNode;
  /** Pulling an empty composer up opens what was sent before; absent when nothing was. */
  onRecent?: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const draft = useDraft(session);
  const [adding, setAdding] = useState(false);
  const source = useRef<Source | null>(null);
  const uploading = attachments.some((attachment) => attachment.upload === 'sending');
  const room = MAX_ATTACHMENTS - attachments.length;
  const [focused, setFocused] = useState(false);
  const input = useRef<TextInput>(null);
  const [sheet, setSheet] = useState(false);
  const typing = useKeyboardShown();
  const configs = sessionConfigs(setup);
  const slots = pickerSlots(configs, provider as never);
  const model = current(slots[0]?.config);
  const effort = current(slots[1]?.config);
  const pulls = !draft && onRecent !== undefined;
  const pullUp = useMemo(
    () =>
      PanResponder.create({
        onMoveShouldSetPanResponderCapture: (_, gesture) => pulls && -gesture.dy > PULL_UP && -gesture.dy > Math.abs(gesture.dx) * 2,
        onPanResponderGrant: () => {
          haptics.select();
          onRecent?.();
        },
      }),
    [pulls, onRecent],
  );
  const dock = requests.length ? (
    <PermissionDock requests={requests} provider={provider} answering={answering} onAnswer={session.answer} />
  ) : null;

  if (watchOnly) {
    return (
      <SafeAreaView edges={typing ? [] : ['bottom']} style={styles.wrap}>
        {dock ?? strip}
        <View style={[styles.composer, styles.watch]}>
          <Icon name="IconEye" size={17} color={colors.inkDim} />
          <Text style={styles.watchText}>
            Watching. This phone can answer permission requests on {hostName}; the host can give it full access.
          </Text>
        </View>
      </SafeAreaView>
    );
  }

  const matches = matchingCommands(commands, draft);
  const sendable = Boolean(draft.trim()) || attachments.length > 0;
  const send = () => {
    const text = draft.trim();
    if (!sendable || offline || uploading) return;
    haptics.tap();
    void session.send(text).then((sent) => {
      if (!sent) return;
      // Clearing the state alone leaves text the keyboard is still composing.
      input.current?.clear();
      session.setDraft('');
      onSent();
    });
  };

  const pick = (from: Source) => {
    const picking = from === 'photos' ? pickPhotos(room) : pickFiles(room);
    picking
      .then(session.attach)
      .catch((error: unknown) => session.report(`could not pick: ${error instanceof Error ? error.message : String(error)}`));
  };
  // iOS shows a picker only once the sheet over the screen has gone.
  const choose = (from: Source) => {
    setAdding(false);
    if (Platform.OS === 'ios') source.current = from;
    else pick(from);
  };
  const dismissed = () => {
    const from = source.current;
    source.current = null;
    if (from) pick(from);
  };

  return (
    <SafeAreaView edges={typing ? [] : ['bottom']} style={styles.wrap}>
      {dock}
      <Commands matches={matches} onPick={(name) => session.setDraft(`/${name} `)} />
      {dock || matches.length ? null : strip}
      <View style={[styles.composer, focused && { borderColor: colors.borderSelected }]} {...pullUp.panHandlers}>
        {attachments.length ? <ComposerAttachments attachments={attachments} onRemove={session.removeAttachment} onRetry={send} /> : null}
        <ComposerInput
          ref={input}
          value={draft}
          editable={!uploading}
          onChangeText={session.setDraft}
          placeholder={placeholder}
          accessibilityLabel="Message"
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        <View style={styles.bar}>
          <Pressable
            onPress={() => setAdding(true)}
            disabled={uploading || room <= 0}
            style={({ pressed }) => [
              styles.add,
              pressed && { backgroundColor: colors.active },
              (uploading || room <= 0) && { opacity: 0.4 },
            ]}
            accessibilityRole="button"
            accessibilityLabel="Add photos or files"
            accessibilityState={{ disabled: uploading || room <= 0 }}>
            <Icon name="IconPlus" size={17} color={colors.inkDim} />
          </Pressable>
          <YoloToggle mode={permissionMode} provider={provider} locked={modeLocked} onToggle={session.setPermissionMode} />
          {model ? (
            <Pressable
              style={[styles.picker, styles.model]}
              onPress={() => setSheet(true)}
              accessibilityRole="button"
              accessibilityLabel={`Model, ${model}`}>
              <AgentIcon provider={provider} size={16} />
              <Text style={[styles.pickerText, styles.modelText, { color: colors.accent }]} numberOfLines={1}>
                {model}
              </Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.accent} />
              </View>
            </Pressable>
          ) : null}
          {effort ? (
            <Pressable
              style={styles.picker}
              onPress={() => setSheet(true)}
              accessibilityRole="button"
              accessibilityLabel={`Effort, ${effort}`}>
              <Text style={styles.pickerText} numberOfLines={1}>
                {effort}
              </Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.inkDim} />
              </View>
            </Pressable>
          ) : null}
          {usage ? <ContextRing usage={usage} provider={provider} onPress={() => setSheet(true)} /> : null}
          <View style={{ flex: 1 }} />
          {sendable || !running ? (
            <Pressable
              onPress={send}
              style={[styles.send, (!sendable || offline) && { opacity: 0.28 }]}
              disabled={offline || uploading}
              accessibilityRole="button"
              accessibilityLabel={
                uploading ? 'Sending files' : offline ? `Send, waiting for ${hostName}` : running ? 'Send after this turn' : 'Send'
              }
              accessibilityState={{ disabled: offline || !sendable, busy: uploading }}>
              {uploading ? (
                <ActivityIndicator size="small" color={colors.ground} />
              ) : (
                <Icon name="IconArrowUp" size={16} color={colors.ground} />
              )}
            </Pressable>
          ) : (
            <Pressable onPress={session.cancel} style={styles.send} accessibilityRole="button" accessibilityLabel="Stop">
              <View style={styles.stop} />
            </Pressable>
          )}
        </View>
      </View>
      <AttachSheet visible={adding} onClose={() => setAdding(false)} onPick={choose} onDismiss={dismissed} />
      <ConfigSheet
        visible={sheet}
        onClose={() => setSheet(false)}
        provider={provider}
        configs={configs}
        usage={usage}
        onPick={(config, value) => {
          session.setConfig(config.id, value);
          setSheet(false);
        }}
      />
    </SafeAreaView>
  );
});

/** What this chat sent before, newest first; a tap puts one back in the composer. */
export function RecentSheet({
  visible,
  prompts,
  onClose,
  onPick,
}: {
  visible: boolean;
  prompts: readonly string[];
  onClose: () => void;
  onPick: (text: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <Text style={styles.sheetLabelFirst}>Sent in this chat</Text>
      <View style={styles.recent}>
        {[...prompts]
          .reverse()
          .slice(0, RECENT_PROMPTS)
          .map((prompt, index) => (
            <Pressable
              key={index}
              onPress={() => onPick(prompt)}
              style={({ pressed }) => [styles.recentRow, pressed && { backgroundColor: colors.active }]}
              accessibilityRole="button"
              accessibilityHint="Puts it in the composer">
              <Icon name="IconClock" size={15} color={colors.inkFaint} />
              <Text style={styles.recentText} numberOfLines={2}>
                {prompt}
              </Text>
            </Pressable>
          ))}
      </View>
    </Sheet>
  );
}

const RECENT_PROMPTS = 20;

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    composer: { padding: 6, borderRadius: 16, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.composer },
    bar: { flexDirection: 'row', alignItems: 'center', gap: 2, paddingTop: 6 },
    add: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
    yolo: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingHorizontal: 8, borderRadius: 7 },
    yoloText: { fontFamily: fonts.uiSemibold, fontSize: 11, letterSpacing: 0.9, textTransform: 'uppercase', color: colors.inkFaint },
    picker: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 34, paddingHorizontal: 7, borderRadius: 7, flexShrink: 0 },
    model: { flexShrink: 1, maxWidth: 160 },
    modelText: { flexShrink: 1 },
    ring: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
    pages: { flexDirection: 'row', alignItems: 'center', gap: 2, marginTop: -2 },
    pageButton: { width: 26, height: 26, borderRadius: 8, alignItems: 'center', justifyContent: 'center' },
    pageText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    cost: { marginTop: 8, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    sheetLabelFirst: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary, paddingBottom: 8, paddingHorizontal: 6 },
    recent: { gap: 2, paddingBottom: 4 },
    recentRow: { flexDirection: 'row', alignItems: 'flex-start', gap: 12, paddingVertical: 11, paddingHorizontal: 8, borderRadius: 9 },
    recentText: { flex: 1, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 20, color: colors.secondary },
    pickerText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    send: { width: 34, height: 34, borderRadius: 17, backgroundColor: colors.ink, alignItems: 'center', justifyContent: 'center' },
    stop: { width: 10, height: 10, borderRadius: 2, backgroundColor: colors.ground },
    watch: { flexDirection: 'row', alignItems: 'center', gap: 10, padding: 14 },
    watchText: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.inkDim },

    dock: {
      marginBottom: 8,
      padding: 12,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      borderRadius: 16,
      backgroundColor: colors.overlay,
    },
    dockHead: { flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
    dockTitle: { fontFamily: fonts.uiSemibold, fontSize: 15, letterSpacing: -0.2, color: colors.ink },
    dockDetail: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, marginTop: 1 },
    dockCmd: {
      marginTop: 10,
      maxHeight: 150,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    dockCmdPad: { paddingVertical: 9, paddingHorizontal: 11 },
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

    commands: { gap: 4, marginBottom: 8 },
    command: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 40,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    commandName: { fontFamily: fonts.mono, fontSize: 13, color: colors.accent },
    commandText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },

    sources: { gap: 8, paddingBottom: 4 },
    source: {
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
    sourceText: { fontFamily: fonts.uiMedium, fontSize: 15.5, color: colors.ink },
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
