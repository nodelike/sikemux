import { describe, expect, it } from 'vitest';

import { earlierMessages, withEarlier } from '@mac/chat/earlier';
import { chatReducer, initialChatState } from '@mac/chat/reducer';
import { runningSubagents } from '@mac/chat/transcript';

import { parsed } from './chatEvents';

const update = (update: Record<string, unknown>) => ({ kind: 'session_update', payload: { sessionId: 's', update } });
const said = (messageId: string, text: string) =>
  update({ sessionUpdate: 'agent_message_chunk', messageId, content: { type: 'text', text } });

function turn(n: number) {
  return [
    { kind: 'prompt', payload: { text: `ask ${n}`, paths: [] } },
    { kind: 'turn_started', payload: {} },
    said(`m${n}`, `answer ${n}`),
    update({ sessionUpdate: 'tool_call', toolCallId: `t${n}`, title: 'ls', status: 'pending' }),
    update({ sessionUpdate: 'tool_call_update', toolCallId: `t${n}`, status: 'completed' }),
    { kind: 'turn_completed', payload: { stopReason: 'end_turn' } },
  ];
}

const reduced = (events: unknown[]) => parsed(JSON.stringify(events)).reduce(chatReducer, initialChatState);
const texts = (messages: { parts: { kind: string; text?: string }[] }[]) =>
  messages.flatMap((message) => message.parts.flatMap((part) => (part.kind === 'text' ? [part.text] : [])));

describe('a chat shown from its last turns', () => {
  it('draws updates whose start the host left out without them', () => {
    const state = reduced([
      update({ sessionUpdate: 'tool_call_update', toolCallId: 'gone', status: 'completed' }),
      update({ sessionUpdate: 'tool_call_update', toolCallId: 'gone-too', title: 'cat', status: 'completed' }),
      said('cut', 'the rest of a message'),
      update({ sessionUpdate: 'subagent_state_update', subagentSessionId: 'nobody', state: 'completed' }),
      update({ sessionUpdate: 'async_task_progress', asyncTaskId: 'nothing' }),
      { kind: 'turn_completed', payload: { stopReason: 'end_turn' } },
      ...turn(9),
    ]);
    expect(texts(state.messages)).toEqual(['the rest of a message', 'ask 9', 'answer 9']);
    expect(state.running).toBe(false);
  });

  it('puts earlier turns above without touching what is on screen', () => {
    const shown = reduced([...turn(2), ...turn(3)]);
    const page = earlierMessages({ number: 1, actions: parsed(JSON.stringify([...turn(0), ...turn(1)])) }, []);
    const state = withEarlier(shown, page);

    expect(texts(state.messages)).toEqual(['ask 0', 'answer 0', 'ask 1', 'answer 1', 'ask 2', 'answer 2', 'ask 3', 'answer 3']);
    expect(state.messages.slice(page.length)).toEqual(shown.messages);
    state.messages.slice(page.length).forEach((message, index) => expect(message).toBe(shown.messages[index]));
    const ids = state.messages.map((message) => message.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(page.every((message) => message.id.startsWith('earlier-1-'))).toBe(true);
    expect(state.revision).toBe(shown.revision + 1);
  });

  it('keeps a tool call and its result together within a page', () => {
    const [, answer] = earlierMessages({ number: 1, actions: parsed(JSON.stringify(turn(0))) }, []);
    const tool = answer.parts.find((part) => part.kind === 'tool');
    expect(tool?.kind === 'tool' && tool.tool.status).toBe('completed');
  });

  it('leaves the chat alone when a page is empty', () => {
    const shown = reduced(turn(0));
    expect(withEarlier(shown, earlierMessages({ number: 1, actions: [] }, []))).toBe(shown);
  });

  it('counts the subagents a whole chat would, once an earlier page joins the end', () => {
    const spawned = (session: string, id: string) => ({
      kind: 'session_update',
      payload: { sessionId: session, update: { sessionUpdate: 'subagent_spawned', subagentSessionId: id, name: id } },
    });
    const ended = (id: string) => update({ sessionUpdate: 'subagent_state_update', subagentSessionId: id, state: 'completed' });
    const page = [...turn(0), spawned('s', 'corpus'), spawned('corpus', 'fixtures'), spawned('s', 'still')];
    // The host starts the end with the starts of the subagents it speaks of.
    const tail = [
      spawned('s', 'corpus'),
      spawned('corpus', 'fixtures'),
      spawned('s', 'still'),
      ...turn(1),
      ended('fixtures'),
      ended('corpus'),
    ];
    const whole = reduced([...page, ...tail]);
    const shown = reduced(tail);
    const stitched = withEarlier(
      shown,
      earlierMessages({ number: 1, actions: parsed(JSON.stringify(page)) }, parsed(JSON.stringify(tail))),
    );

    const names = (state: typeof whole) => runningSubagents(state.messages).map((subagent) => subagent.name);
    expect(names(whole)).toEqual(['still']);
    expect(names(shown)).toEqual(['still']);
    expect(names(stitched)).toEqual(['still']);
    const cards = stitched.messages.flatMap((message) => message.parts.filter((part) => part.kind === 'subagent'));
    expect(cards).toHaveLength(2);
  });
});
