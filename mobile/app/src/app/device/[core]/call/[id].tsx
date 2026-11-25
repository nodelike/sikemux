import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import { KeyboardStickyView } from 'react-native-keyboard-controller';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import type { ConnectionLike } from '@sikemux/native';

import { CALLS, composerHint, hidden, NO_CALLS, pageTitle } from '@shared/calls/words';
import { useExternalValue } from '@shared/lib/externalValue';
import type { PersonCall } from '@shared/wire.generated';
import { CallBody, readyOf } from '@/calls/CallBody';
import { answer, answerOf, chosenIn, draftOf, edit, openCalls, useKept } from '@/calls/hostCalls';
import { SECRET_INPUT } from '@/calls/secretInput';
import { ComposerInput } from '@/chat/ComposerInput';
import { connectionOf, useDevices, useLive } from '@/devices/hub';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Nav, Screen } from '@/ui/screen';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** One call on a page of its own, as the Mac opens a call in its own tab, with the composer it is answered with. */
export default function CallScreen() {
  const { core, id } = useLocalSearchParams<{ core: string; id: string }>();
  const styles = useStyles(makeStyles);
  const t = useT();
  const live = useLive(core);
  const { devices } = useDevices();
  const watchOnly = devices.find((paired) => paired.core === core)?.access === 'watch';
  const canAnswer = !watchOnly && live.status === 'open';
  const open = openCalls(live.snapshot);
  const waiting = open.find((call) => call.id === id);
  const [shown, setShown] = useState(waiting);
  const [refused, setRefused] = useState<string>();
  const homeBar = useSafeAreaInsets().bottom;
  if (waiting && waiting !== shown) setShown(waiting);
  const call = waiting ?? shown;
  const answered = !waiting && !!shown;

  useEffect(() => {
    if (answered && router.canGoBack()) router.back();
  }, [answered]);

  const at = call ? open.findIndex((other) => other.id === call.id) : -1;
  const next = at >= 0 && open.length > 1 ? open[(at + 1) % open.length] : undefined;
  const connection = connectionOf(live);

  return (
    <Screen>
      <Nav title={call ? pageTitle(call) : t(CALLS)} />
      {call ? (
        <>
          <ScrollView
            style={{ flex: 1 }}
            contentContainerStyle={styles.content}
            keyboardDismissMode="on-drag"
            keyboardShouldPersistTaps="handled">
            <CallBody
              core={core}
              calls={live.snapshot?.calls}
              call={call}
              next={next}
              connection={connection}
              canAnswer={canAnswer}
              report={setRefused}
            />
          </ScrollView>
          <KeyboardStickyView offset={{ opened: homeBar }}>
            <CallComposer
              core={core}
              calls={live.snapshot?.calls}
              call={call}
              connection={connection}
              canAnswer={canAnswer}
              refused={refused}
              report={setRefused}
            />
          </KeyboardStickyView>
        </>
      ) : (
        <Text style={styles.none}>{t(NO_CALLS)}</Text>
      )}
    </Screen>
  );
}

/**
 * The chat's composer box at the bottom of a call's page, holding the note that goes with the choice made or, with none
 * made, the person's own answer. A call for a secret takes it hidden.
 */
function CallComposer({
  core,
  calls,
  call,
  connection,
  canAnswer,
  refused,
  report,
}: {
  core: string;
  calls: readonly PersonCall[] | undefined;
  call: PersonCall;
  connection: ConnectionLike | undefined;
  canAnswer: boolean;
  refused: string | undefined;
  report: (text: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const held = useKept(core);
  const store = draftOf(calls, call);
  const draft = useExternalValue(store);
  const [focused, setFocused] = useState(false);
  const seen = call.kind !== 'production' || held.seen.has(call.id);
  const ready = readyOf(call, draft, canAnswer, seen);
  const hint = composerHint(call, chosenIn(draft, call), t);
  const secret = !!call.secret;

  const send = () => {
    const params = answerOf(store.get(), call);
    if (!params || !ready) return;
    haptics.select();
    void answer(core, connection, call, store, params, report);
  };

  return (
    <SafeAreaView edges={['bottom']} style={styles.wrap}>
      {refused && !draft.sending ? <Text style={styles.refused}>{refused}</Text> : null}
      <View style={[styles.box, { borderColor: focused ? colors.borderSelected : colors.border }]}>
        <View style={styles.line}>
          <View style={{ flex: 1 }}>
            {secret ? (
              <TextInput
                value={draft.typed}
                onChangeText={(text) => edit(store, { typed: text })}
                placeholder={hint}
                placeholderTextColor={colors.inkFaint}
                // Its words never show, and the keyboard neither suggests nor remembers them. A hidden secure field
                // cannot hold the line breaks of a pasted key, so the text is drawn clear; paste still works.
                style={[styles.secret, draft.typed ? styles.hiddenText : null]}
                multiline
                {...SECRET_INPUT}
                returnKeyType="send"
                submitBehavior="submit"
                onSubmitEditing={send}
                caretHidden={!!draft.typed}
                editable={canAnswer}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
              />
            ) : (
              <ComposerInput
                value={draft.typed}
                onChangeText={(text) => edit(store, { typed: text })}
                placeholder={hint}
                editable={canAnswer}
                accessibilityLabel={t('Message')}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
              />
            )}
          </View>
          <Pressable
            onPress={send}
            disabled={!ready}
            style={[styles.send, !ready && { opacity: 0.28 }]}
            accessibilityRole="button"
            accessibilityLabel={t('Send')}
            accessibilityState={{ disabled: !ready, busy: draft.sending }}>
            {draft.sending ? (
              <ActivityIndicator size="small" color={colors.ground} />
            ) : (
              <Icon name="IconArrowUp" size={16} color={colors.ground} />
            )}
          </Pressable>
        </View>
        {secret && draft.typed ? <Text style={styles.hiddenSize}>{hidden(draft.typed, t)}</Text> : null}
      </View>
    </SafeAreaView>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    content: { paddingHorizontal: 18, paddingTop: 8, paddingBottom: 24 },
    none: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingVertical: 24, textAlign: 'center' },
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    refused: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.danger, paddingHorizontal: 8, paddingBottom: 6 },
    box: { padding: 6, borderRadius: 10, borderWidth: 1, backgroundColor: colors.composer },
    line: { flexDirection: 'row', alignItems: 'flex-end', gap: 4 },
    send: {
      width: 34,
      height: 34,
      marginBottom: 4,
      borderRadius: 17,
      backgroundColor: colors.ink,
      alignItems: 'center',
      justifyContent: 'center',
    },
    secret: { minHeight: 44, paddingHorizontal: 10, paddingTop: 9, fontFamily: fonts.ui, fontSize: 14.5, color: colors.ink },
    hiddenText: { color: 'transparent' },
    hiddenSize: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim, paddingHorizontal: 10, paddingBottom: 4 },
  });
};
