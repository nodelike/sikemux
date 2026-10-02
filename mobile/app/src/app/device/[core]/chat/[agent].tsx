import { useEffect, useRef, useState } from 'react';
import { KeyboardAvoidingView, Pressable, StyleSheet, Text, View, type NativeScrollEvent, type NativeSyntheticEvent } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { FlashList, type FlashListRef } from '@shopify/flash-list';

import { activityText, composerPlaceholder } from '@mac/chat/chatStatus';
import { activeToolLabel } from '@mac/chat/toolLabels';
import type { ChatMessage } from '@mac/chat/types';
import { Composer } from '@/chat/Composer';
import { Activity, Message, Queued } from '@/chat/Transcript';
import { useChat } from '@/chat/useChat';
import { useDevices, useLive } from '@/devices/hub';
import { chatTitle, providerName } from '@/devices/words';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button, Nav, Screen, useBottomGap, Working } from '@/ui/parts';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

/** How near the end, as a share of the transcript's height, still counts as reading the latest. */
const FOLLOW = 0.1;

/** When the running turn began, as this phone saw it, for the working line's clock. */
function useTurnStart(running: boolean): number {
  const [since, setSince] = useState(() => Date.now());
  const was = useRef(running);
  useEffect(() => {
    if (running && !was.current) setSince(Date.now());
    was.current = running;
  }, [running]);
  return since;
}

function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1) + (text.endsWith('.') ? '' : '.');
}

export default function Chat() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { core, agent } = useLocalSearchParams<{ core: string; agent: string }>();
  const live = useLive(core);
  const { devices } = useDevices();
  const access = devices.find((device) => device.core === core)?.access ?? 'full';
  const info = live.snapshot?.chats.find((chat) => chat.agentId === agent);
  const provider = info?.provider ?? 'agent';
  const chat = useChat(core, agent);
  const { state } = chat;
  const since = useTurnStart(state.running);
  const list = useRef<FlashListRef<ChatMessage>>(null);
  const [away, setAway] = useState(false);
  const bottom = useBottomGap();
  const onScroll = ({ nativeEvent }: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = nativeEvent;
    setAway(contentSize.height - contentOffset.y - layoutMeasurement.height > layoutMeasurement.height * FOLLOW);
  };
  const send = (text: string) => {
    chat.send(text);
    requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
  };
  const activity = activityText(state, activeToolLabel(state.messages));
  const title = state.title ?? (info ? chatTitle(info) : providerName(provider));
  const status =
    chat.attached === 'missing' ? (
      <View style={styles.missing}>
        <Text style={styles.gone}>{chat.problem ? capitalised(chat.problem) : 'This chat is no longer running on the Mac.'}</Text>
        <Button title="Try again" onPress={chat.retry} />
      </View>
    ) : chat.attached === 'attaching' ? (
      <View style={styles.attaching}>
        <Working />
        <Text style={type.meta}>{info?.asleep ? 'Waking the chat…' : 'Opening the chat…'}</Text>
      </View>
    ) : null;

  return (
    <Screen>
      <Nav
        title={
          <>
            <AgentIcon provider={provider} size={17} />
            <Text style={styles.title} numberOfLines={1}>
              {title}
            </Text>
          </>
        }
      />
      <KeyboardAvoidingView style={{ flex: 1 }} behavior="padding">
        {chat.attached !== 'live' && !state.messages.length ? (
          <View style={[styles.transcript, styles.alone, { paddingBottom: bottom }]}>{status}</View>
        ) : (
          <View style={styles.transcript}>
            <FlashList
              ref={list}
              data={state.messages}
              keyExtractor={(message) => message.id}
              getItemType={(message) => message.role}
              extraData={chat.replayed}
              renderItem={({ item }) => <Message message={item} untimed={chat.replayed.has(item.id)} />}
              ListHeaderComponent={chat.attached === 'live' ? null : status}
              ListFooterComponent={
                <>
                  {chat.queued ? <Queued text={chat.queued} /> : null}
                  {activity ? <Activity provider={provider} label={activity} since={since} /> : null}
                  {state.error ? <Text style={styles.error}>{state.error}</Text> : null}
                </>
              }
              contentContainerStyle={{ ...styles.content, ...(chat.attached !== 'live' && { paddingBottom: bottom }) }}
              maintainVisibleContentPosition={{
                startRenderingFromBottom: true,
                autoscrollToBottomThreshold: FOLLOW,
                animateAutoScrollToBottom: false,
              }}
              onScroll={onScroll}
              scrollEventThrottle={100}
              keyboardDismissMode="interactive"
            />
            {away ? (
              <Pressable
                onPress={() => list.current?.scrollToEnd({ animated: true })}
                style={styles.jump}
                hitSlop={8}
                accessibilityRole="button"
                accessibilityLabel="Jump to latest message">
                <Icon name="IconArrowDown" size={16} color={colors.inkDim} />
              </Pressable>
            ) : null}
          </View>
        )}
        {chat.attached === 'live' ? (
          <Composer
            state={state}
            provider={provider}
            placeholder={composerPlaceholder(state, { resuming: false, disconnected: live.status !== 'open' })}
            permissionMode={info?.permissionMode ?? ''}
            watchOnly={access === 'watch'}
            offline={!chat.connected}
            onSend={send}
            onStop={chat.cancel}
            onAnswer={chat.answer}
            onConfig={chat.setConfig}
          />
        ) : null}
      </KeyboardAvoidingView>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    title: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink, flexShrink: 1 },
    transcript: { flex: 1, borderTopWidth: 1, borderTopColor: colors.border },
    alone: { justifyContent: 'flex-end' },
    content: { paddingHorizontal: 18, paddingTop: 6, paddingBottom: 12 },
    jump: {
      position: 'absolute',
      bottom: 8,
      alignSelf: 'center',
      width: 34,
      height: 34,
      borderRadius: 17,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.composer,
      alignItems: 'center',
      justifyContent: 'center',
      shadowColor: '#000',
      shadowOpacity: 0.4,
      shadowRadius: 12,
      shadowOffset: { width: 0, height: 6 },
      elevation: 6,
    },
    attaching: { flexDirection: 'row', alignItems: 'center', gap: 10, alignSelf: 'center', paddingVertical: 24 },
    missing: { gap: 16, paddingHorizontal: 18, paddingVertical: 24 },
    gone: { ...type.meta, textAlign: 'center' },
    error: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger, marginTop: 10 },
  });
};
