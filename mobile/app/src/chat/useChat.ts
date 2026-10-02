import { useCallback, useEffect, useEffectEvent, useReducer, useRef, useState } from 'react';
import { ChatAttachment, MobileError, type ChatMark } from '@sikemux/native';

import { permissionRequest, promptAction, recordOf, statusFromEvent } from '@mac/chat/acpEvents';
import { chatReducer, initialChatState } from '@mac/chat/reducer';
import type { ChatAction, ChatState } from '@mac/chat/types';

import type { CoreChatEvent } from '@/core/protocol';
import { onChatEvents, problem as problemOf, useLive, type ChatDelivery } from '@/devices/hub';

/** The same mapping the Mac's useAcpSession does from a core chat event to the reducer. */
function actions(event: CoreChatEvent): ChatAction[] {
  const payload = event.payload;
  switch (event.kind) {
    case 'status':
      return [{ type: 'status', state: statusFromEvent({ payload } as never) }];
    case 'ready':
      return [{ type: 'ready', capabilities: recordOf(payload.capabilities) ?? {}, setup: recordOf(payload.setup) ?? {} }];
    case 'session_update': {
      const rows = Array.isArray(payload.updates) ? payload.updates : [payload];
      return rows.flatMap((entry): ChatAction[] => {
        const row = recordOf(entry);
        const update = row && recordOf(row.update);
        return update && typeof row.sessionId === 'string' ? [{ type: 'session_update', sessionId: row.sessionId, update }] : [];
      });
    }
    case 'prompt': {
      const prompted = promptAction(payload);
      return prompted ? [prompted] : [];
    }
    case 'turn_started':
      return [{ type: 'turn_started' }];
    case 'turn_completed':
      return [{ type: 'turn_completed', stopReason: typeof payload.stopReason === 'string' ? payload.stopReason : undefined }];
    case 'permission_request': {
      const request = permissionRequest(payload);
      return request ? [{ type: 'permission_requested', request }] : [];
    }
    case 'error':
      return [{ type: 'error', message: typeof payload.message === 'string' ? payload.message : 'The agent stopped.' }];
    default:
      return [];
  }
}

function parsed(json: string): ChatAction[] {
  return (JSON.parse(json) as CoreChatEvent[]).flatMap(actions);
}

type Change = { type: 'apply'; actions: ChatAction[] } | { type: 'replace'; state: ChatState };

function reduce(state: ChatState, change: Change): ChatState {
  return change.type === 'replace' ? change.state : change.actions.reduce(chatReducer, state);
}

function reduceAll(state: ChatState, batch: ChatAction[]): ChatState {
  return batch.reduce(chatReducer, state);
}

export type ChatView = {
  state: ChatState;
  /** Messages rebuilt from the replay, which carries no times. */
  replayed: ReadonlySet<string>;
  attached: 'attaching' | 'live' | 'missing';
  /** The Mac is reachable, so what the person sends can arrive. */
  connected: boolean;
  /** Why the chat could not be opened, in the Mac's words. */
  problem: string | null;
  queued: string | null;
  send: (text: string) => void;
  cancel: () => void;
  answer: (requestId: string, optionId: string | null) => void;
  setConfig: (configId: string, value: string) => void;
  retry: () => void;
};

const TOO_LONG = 'This chat is longer than the Mac keeps for the phone. Open it on the Mac to carry on.';

export function useChat(core: string, agentId: string): ChatView {
  const live = useLive(core);
  const [state, change] = useReducer(reduce, initialChatState);
  const [attached, setAttached] = useState<ChatView['attached']>('attaching');
  const [problem, setProblem] = useState<string | null>(null);
  const [queued, setQueued] = useState<string | null>(null);
  const [replayed, setReplayed] = useState<ReadonlySet<string>>(new Set());
  const [attempt, setAttempt] = useState(0);
  const connection = live.status === 'open' ? live.connection : undefined;
  const connectionRef = useRef(connection);
  /** Where this chat's events got to, so a reconnect asks only for what it missed. */
  const mark = useRef<ChatMark | undefined>(undefined);
  const [resumable, setResumable] = useState(false);

  const [run, setRun] = useState({ connection, core, agentId, attempt });
  if (run.connection !== connection || run.core !== core || run.agentId !== agentId || run.attempt !== attempt) {
    setRun({ connection, core, agentId, attempt });
    const sameChat = run.agentId === agentId;
    if (!sameChat) setResumable(false);
    if (connection) {
      if (!(sameChat && resumable)) setAttached('attaching');
      setProblem(null);
    }
  }

  useEffect(() => {
    connectionRef.current = connection;
  }, [connection]);

  useEffect(() => {
    mark.current = undefined;
  }, [agentId]);

  useEffect(() => {
    if (!connection) return;
    let current = true;
    let held: ChatDelivery[] | undefined = [];
    let pending: ChatAction[] = [];
    let frame: number | undefined;

    // A stream of tokens is drawn once a frame, not once an event.
    const flush = () => {
      frame = undefined;
      if (!current || !pending.length) return;
      const batch = pending;
      pending = [];
      change({ type: 'apply', actions: batch });
    };
    const take = (deliveries: ChatDelivery[]) => {
      const after = mark.current?.seq ?? BigInt(-1);
      for (const delivery of deliveries) {
        if (delivery.seq <= after) continue;
        pending.push(...actions(JSON.parse(delivery.eventJson) as CoreChatEvent));
        if (mark.current) mark.current = { ...mark.current, seq: delivery.seq };
      }
      if (frame === undefined) frame = requestAnimationFrame(flush);
    };

    const off = onChatEvents(core, (deliveries) => {
      const mine = deliveries.filter((delivery) => delivery.agentId === agentId);
      if (!mine.length) return;
      // Until the attach answer says where the replay ends, events wait.
      if (held) held.push(...mine);
      else take(mine);
    });

    // A chat the Mac put to sleep starts again first; one already running answers at once.
    connection
      .wakeChat(agentId)
      .then(() => connection.attachChat(agentId, mark.current))
      .then((attachment) => {
        if (!current) return;
        if (ChatAttachment.Live.instanceOf(attachment)) {
          const { inner } = attachment;
          const replay = parsed(inner.replayJson);
          const rebuilt = reduceAll(initialChatState, [
            ...replay,
            {
              type: 'ready',
              capabilities: recordOf(JSON.parse(inner.capabilitiesJson)) ?? {},
              setup: recordOf(JSON.parse(inner.setupJson)) ?? {},
            },
            ...(inner.running ? [{ type: 'turn_started' } as const] : []),
          ]);
          setReplayed(new Set(reduceAll(initialChatState, replay).messages.map((message) => message.id)));
          change({ type: 'replace', state: rebuilt });
          mark.current = inner.mark;
          setResumable(true);
        } else if (ChatAttachment.Resumed.instanceOf(attachment)) {
          pending.push(...parsed(attachment.inner.eventsJson));
          mark.current = attachment.inner.mark;
          setResumable(true);
        } else {
          setProblem(ChatAttachment.Restart.instanceOf(attachment) ? TOO_LONG : null);
          setAttached('missing');
          return;
        }
        const waiting = held ?? [];
        held = undefined;
        take(waiting);
        setAttached('live');
      })
      .catch((error: unknown) => {
        if (!current) return;
        setProblem(MobileError.Refused.instanceOf(error) ? error.inner.message : problemOf(error));
        setAttached('missing');
      });

    return () => {
      current = false;
      off();
      if (frame !== undefined) cancelAnimationFrame(frame);
      if (connection.isOpen()) connection.detachChat(agentId).catch(() => {});
    };
  }, [connection, core, agentId, attempt]);

  const fail = useCallback((what: string, error: unknown) => {
    change({ type: 'apply', actions: [{ type: 'error', message: `${what}: ${problemOf(error)}` }] });
  }, []);

  const withConnection = useCallback(() => {
    const open = connectionRef.current;
    if (!open) throw new Error('the Mac is not connected');
    return open;
  }, []);

  const deliver = useCallback(
    (text: string) => {
      Promise.resolve()
        .then(() => withConnection().prompt(agentId, text))
        .catch((error: unknown) => fail('Not sent', error));
    },
    [agentId, fail, withConnection],
  );

  const prompt = useCallback(
    (text: string) => {
      change({ type: 'apply', actions: [{ type: 'local_prompt', text, paths: [] }] });
      deliver(text);
    },
    [deliver],
  );

  const running = state.running;
  const [outbox, setOutbox] = useState<{ text: string } | null>(null);
  if (!running && queued !== null) {
    setQueued(null);
    change({ type: 'apply', actions: [{ type: 'local_prompt', text: queued, paths: [] }] });
    setOutbox({ text: queued });
  }
  const deliverQueued = useEffectEvent((text: string) => deliver(text));
  useEffect(() => {
    if (outbox) deliverQueued(outbox.text);
  }, [outbox]);

  const send = useCallback(
    (text: string) => {
      if (running) setQueued((held) => (held ? `${held}\n\n${text}` : text));
      else prompt(text);
    },
    [running, prompt],
  );

  const cancel = useCallback(() => {
    Promise.resolve()
      .then(() => withConnection().cancel(agentId))
      .catch((error: unknown) => fail('Could not stop the turn', error));
  }, [agentId, fail, withConnection]);

  const answer = useCallback(
    (requestId: string, optionId: string | null) => {
      Promise.resolve()
        .then(() => withConnection().answerPermission(agentId, requestId, optionId ?? undefined))
        .then(() => change({ type: 'apply', actions: [{ type: 'permission_cleared', requestId }] }))
        .catch((error: unknown) => fail('The answer did not reach the Mac', error));
    },
    [agentId, fail, withConnection],
  );

  const setConfig = useCallback(
    (configId: string, value: string) => {
      Promise.resolve()
        .then(() => withConnection().setChatConfig(agentId, configId, value))
        .then((json) => {
          const options = recordOf(JSON.parse(json))?.configOptions;
          if (options) change({ type: 'apply', actions: [{ type: 'config', options }] });
        })
        .catch((error: unknown) => fail('Could not change the setting', error));
    },
    [agentId, fail, withConnection],
  );

  const retry = useCallback(() => {
    mark.current = undefined;
    setResumable(false);
    setAttempt((count) => count + 1);
  }, []);

  return { state, replayed, attached, connected: connection !== undefined, problem, queued, send, cancel, answer, setConfig, retry };
}
