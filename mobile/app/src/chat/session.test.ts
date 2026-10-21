import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatAttachment, MobileError, type ConnectionLike } from '@sikemux/native';

import type { ChatInfo, ChatState } from '@/core/protocol';
import type { ChatDelivery } from '@/devices/hub';
import { ChatSession } from './session';
import type { Attachment } from './uploads';

const FEED = 'feed-1';
const event = (kind: string, payload: Record<string, unknown> = {}) => ({ kind, payload });
const said = (text: string) =>
  event('session_update', {
    sessionId: 's',
    update: { sessionUpdate: 'agent_message_chunk', messageId: 'm', content: { type: 'text', text } },
  });
const asking = (requestId: string) =>
  event('permission_request', {
    requestId,
    sessionId: 's',
    toolCall: { toolCallId: `t-${requestId}`, title: 'rm -rf build', kind: 'execute' },
    options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }],
  });

function live(events: unknown[], seq: number, running = false) {
  return ChatAttachment.Live.new({
    sessionId: 's',
    capabilitiesJson: '{}',
    setupJson: '{}',
    permissionMode: 'default',
    running,
    turned: false,
    replayJson: JSON.stringify(events),
    mark: { feed: FEED, seq: BigInt(seq) },
    olderBefore: undefined,
  });
}

const queued = (...messages: { id: string; text: string; paths: string[] }[]) =>
  event('session_update', {
    sessionId: 's',
    update: { sessionUpdate: 'queued_messages', messages: messages.map((message) => ({ ...message, context: [] })) },
  });

function fakeConnection(attachment: unknown) {
  return {
    isOpen: () => true,
    wakeChat: vi.fn(async () => {}),
    attachChat: vi.fn(async (_agent: string, _since?: { feed: string; seq: bigint }) => attachment),
    detachChat: vi.fn(async () => {}),
    prompt: vi.fn(async (_agent: string, _text: string, _paths: string[]) => {}),
    attachFile: vi.fn(async (_agent: string, name: string, _mime: string, _bytes: ArrayBuffer) => `/host/pasted/${name}`),
    cancel: vi.fn(async () => {}),
    steer: vi.fn(async (_agent: string, _text: string, _paths: string[]) => true),
    call: vi.fn(async (_method: string, _params: string) => 'null'),
    stopTask: vi.fn(async (_agent: string, _task: string) => {}),
    setPermissionMode: vi.fn(async (_agent: string, _mode: string) => {}),
    answerPermission: vi.fn(async () => {}),
    setChatConfig: vi.fn(async () => '{}'),
    chatHistory: vi.fn(),
  };
}

function harness() {
  let take: ((deliveries: ChatDelivery[]) => void) | undefined;
  const frames: (() => void)[] = [];
  const session = new ChatSession('agent', {
    read: async () => new ArrayBuffer(4),
    listen: (listen) => {
      take = listen;
      return () => {
        take = undefined;
      };
    },
    frame: (run) => {
      frames.push(run);
      return () => frames.splice(frames.indexOf(run), 1);
    },
  });
  let seq = 0;
  const deliver = (...events: unknown[]) =>
    take?.(events.map((payload) => ({ agentId: 'agent', seq: BigInt((seq += 1)), eventJson: JSON.stringify(payload) })));
  const startAt = (value: number) => {
    seq = value;
  };
  const drawFrame = () => frames.splice(0).forEach((run) => run());
  return { session, deliver, startAt, drawFrame };
}

let picks = 0;
const picked = (name: string, kind: 'image' | 'file', size = 1000): Attachment => ({
  id: `pick-${(picks += 1)}`,
  kind,
  name,
  mime: kind === 'image' ? 'image/jpeg' : 'application/octet-stream',
  uri: `file:///picked/${name}`,
  size,
});

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const texts = (session: ChatSession) =>
  session.snapshot().agent.messages.flatMap((message) => message.parts.flatMap((part) => (part.kind === 'text' ? [part.text] : [])));
const info = (state: ChatState, asleep = false) => ({ agentId: 'agent', state, asleep }) as ChatInfo;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('a chat session', () => {
  it('keeps events that arrived but were not drawn when the chat is left', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 5));
    const release = session.hold(connection as unknown as ConnectionLike);
    await settle();
    startAt(5);
    deliver(event('turn_started'), said('half'));
    expect(session.snapshot().agent.running).toBe(false);

    release();
    expect(texts(session)).toEqual(['half']);
    expect(session.snapshot().agent.running).toBe(true);

    const resumed = fakeConnection(
      ChatAttachment.Resumed.new({ eventsJson: JSON.stringify([event('turn_completed')]), mark: { feed: FEED, seq: 8n } }),
    );
    session.hold(resumed as unknown as ConnectionLike);
    await settle();
    expect(resumed.attachChat.mock.calls[0][1]).toEqual({ feed: FEED, seq: 7n });
    expect(session.snapshot().agent.running).toBe(false);
    drawFrame();
  });

  it('moves the mark only once events are drawn', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const first = fakeConnection(live([], 2));
    session.hold(first as unknown as ConnectionLike);
    await settle();
    startAt(2);
    deliver(said('one'));
    expect(texts(session)).toEqual([]);
    drawFrame();
    expect(texts(session)).toEqual(['one']);

    session.hostSaw(info('stopped'), []);
    session.hostSaw(info('ready'), []);
    await settle();
    expect(first.attachChat.mock.calls[1][1]).toEqual({ feed: FEED, seq: 3n });
  });

  it('marks a message the host did not take, and sends it again on Retry', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    connection.prompt.mockRejectedValueOnce(MobileError.Connection.new({ message: 'the connection dropped' }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.send('hello');
    expect(session.snapshot().agent.running).toBe(true);
    await settle();
    const [[id, unsent]] = [...session.snapshot().unsent];
    expect(unsent).toEqual({ state: 'failed', problem: 'the connection dropped' });
    expect(session.snapshot().agent.running).toBe(false);
    expect(texts(session)).toEqual(['hello']);

    session.retrySend(id);
    expect(session.snapshot().unsent.get(id)?.state).toBe('sending');
    await settle();
    expect(session.snapshot().unsent.size).toBe(0);
    expect(connection.prompt).toHaveBeenCalledTimes(2);
  });

  it('keeps a message it could not send through a full replay', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([event('prompt', { text: 'before' }), event('turn_completed')], 2));
    connection.prompt.mockRejectedValueOnce(new Error('timed out'));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.send('lost');
    await settle();

    session.retry();
    await settle();
    expect(texts(session)).toEqual(['before', 'lost']);
    const [[id, unsent]] = [...session.snapshot().unsent];
    expect(unsent.state).toBe('failed');
    expect(session.snapshot().agent.messages.at(-1)?.id).toBe(id);
  });

  it('shows only the permission requests the host still waits on', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([event('turn_started'), asking('old'), asking('open')], 3, true));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    const ids = () => session.snapshot().permissions.map((request) => request.requestId);
    expect(ids()).toEqual(['old', 'open']);

    session.hostSaw(info('ready'), ['open']);
    expect(ids()).toEqual(['open']);

    startAt(3);
    deliver(asking('new'));
    drawFrame();
    expect(ids()).toEqual(['open', 'new']);
    vi.setSystemTime(Date.now() + 3000);
    session.hostSaw(info('ready'), ['open', 'other']);
    expect(ids()).toEqual(['open']);
  });

  it('keeps a failed answer apart from the agent', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([event('turn_started'), asking('ask')], 2, true));
    connection.answerPermission.mockRejectedValueOnce(new Error('timed out'));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.answer('ask', 'allow');
    session.answer('ask', 'allow');
    expect(session.snapshot().answering.has('ask')).toBe(true);
    await settle();
    expect(connection.answerPermission).toHaveBeenCalledTimes(1);
    const { agent, notice, permissions, answering } = session.snapshot();
    expect(notice).toBe('The answer did not reach the host: timed out');
    expect(agent.running).toBe(true);
    expect(agent.error).toBeNull();
    expect(permissions).toHaveLength(1);
    expect(answering.size).toBe(0);
  });

  it("hands a message written mid-turn to the host's queue, and shows what waits there", async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([event('turn_started')], 1, true));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    await session.send('next');
    await settle();
    expect(connection.prompt).toHaveBeenCalledWith('agent', 'next', []);
    expect(texts(session)).toEqual([]);

    startAt(1);
    deliver(queued({ id: 'q1', text: 'next', paths: [] }));
    drawFrame();
    expect(session.snapshot().queued.map((held) => held.text)).toEqual(['next']);
  });

  it('hands what waited back to the composer on Stop', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([event('turn_started')], 1, true));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.setDraft('and also');
    startAt(1);
    deliver(queued({ id: 'q1', text: 'next', paths: [] }));
    drawFrame();

    session.cancel();
    await settle();
    expect(connection.cancel).toHaveBeenCalled();
    expect(session.draftText()).toBe('next\n\nand also');
  });

  it('asks the host to steer waiting messages into the turn, or to take one back', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([event('turn_started')], 1, true));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.steer(['q1']);
    session.dropQueued('q2');
    await settle();
    expect(connection.call).toHaveBeenCalledWith('steerQueued', JSON.stringify({ agentId: 'agent', ids: ['q1'] }));
    expect(connection.call).toHaveBeenCalledWith('dropQueued', JSON.stringify({ agentId: 'agent', ids: ['q2'] }));
  });

  it('keeps the permission mode it set until the host says otherwise', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    session.hold(connection as unknown as ConnectionLike);
    session.hostSaw({ ...info('ready'), permissionMode: 'workspace-write' }, []);
    await settle();
    expect(session.snapshot().permissionMode).toBe('workspace-write');

    session.setPermissionMode('bypass');
    expect(session.snapshot().changingMode).toBe(true);
    await settle();
    expect(connection.setPermissionMode).toHaveBeenCalledWith('agent', 'bypass');
    expect(session.snapshot()).toMatchObject({ permissionMode: 'bypass', changingMode: false });

    session.hostSaw({ ...info('ready'), permissionMode: 'workspace-write' }, []);
    expect(session.snapshot().permissionMode).toBe('bypass');
    session.hostSaw({ ...info('ready'), permissionMode: 'default' }, []);
    expect(session.snapshot().permissionMode).toBe('default');
  });

  it('stops a background task once', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.stopTask('shell');
    session.stopTask('shell');
    expect(session.snapshot().stopping.has('shell')).toBe(true);
    await settle();
    expect(connection.stopTask).toHaveBeenCalledTimes(1);
    expect(session.snapshot().stopping.size).toBe(0);
  });

  it('puts the picked files on the host before sending their paths with the message', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('shot.jpg', 'image'), picked('notes.txt', 'file')]);

    const sending = session.send('look');
    expect(session.snapshot().attachments.map((attachment) => attachment.upload)).toEqual(['sending', 'sending']);
    expect(session.snapshot().agent.messages).toHaveLength(0);
    expect(await sending).toBe(true);
    expect(connection.attachFile.mock.calls.map((call) => call[1])).toEqual(['shot.jpg', 'notes.txt']);
    expect(connection.prompt).toHaveBeenCalledWith('agent', 'look', ['/host/pasted/shot.jpg', '/host/pasted/notes.txt']);
    const { agent, attachments, sentFiles } = session.snapshot();
    expect(attachments).toHaveLength(0);
    expect(agent.messages.at(-1)?.attachments).toEqual(['/host/pasted/shot.jpg', '/host/pasted/notes.txt']);
    expect(sentFiles.get('/host/pasted/shot.jpg')?.uri).toBe('file:///picked/shot.jpg');
  });

  it('holds the message while a file did not reach the host, and sends only the rest again', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    connection.attachFile.mockRejectedValueOnce(MobileError.Connection.new({ message: 'the connection dropped' }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('a.jpg', 'image'), picked('b.pdf', 'file')]);

    expect(await session.send('both')).toBe(false);
    const [first, second] = session.snapshot().attachments;
    expect(first).toMatchObject({ upload: 'failed', problem: 'the connection dropped' });
    expect(second.path).toBe('/host/pasted/b.pdf');
    expect(connection.prompt).not.toHaveBeenCalled();

    expect(await session.send('both')).toBe(true);
    expect(connection.attachFile).toHaveBeenCalledTimes(3);
    expect(connection.prompt).toHaveBeenCalledWith('agent', 'both', ['/host/pasted/a.jpg', '/host/pasted/b.pdf']);
  });

  it("leaves out files over the host's limit and says so", () => {
    const { session } = harness();
    session.attach([picked('movie.mov', 'file', 11 * 1024 * 1024), picked('ok.png', 'image')]);
    const { attachments, notice } = session.snapshot();
    expect(attachments.map((attachment) => attachment.name)).toEqual(['ok.png']);
    expect(notice).toBe('movie.mov is over 10 MB, too big to send');
  });

  it('hands held files back to the composer on Stop', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([event('turn_started')], 1, true));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('shot.jpg', 'image')]);
    await session.send('next');
    startAt(1);
    deliver(queued({ id: 'q1', text: 'next', paths: ['/host/pasted/shot.jpg'] }));
    drawFrame();

    session.cancel();
    expect(session.snapshot().attachments.map((attachment) => attachment.path)).toEqual(['/host/pasted/shot.jpg']);
    await session.send('next');
    expect(connection.attachFile).toHaveBeenCalledTimes(1);
  });

  it('takes the chat up again when the host brings it back', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    session.hold(connection as unknown as ConnectionLike);
    session.hostSaw(info('ready', true), []);
    await settle();
    session.hostSaw(info('ready'), []);
    expect(connection.attachChat).toHaveBeenCalledTimes(1);

    session.hostSaw(undefined, []);
    session.hostSaw(info('ready'), []);
    await settle();
    expect(connection.attachChat).toHaveBeenCalledTimes(2);
    expect(connection.detachChat).toHaveBeenCalledTimes(1);
  });
});
