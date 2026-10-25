import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatAttachment, MobileError, type ConnectionLike } from '@sikemux/native';

import type { ChatChange, ChatMessage, PermissionAsk } from '@shared/chat/types';
import type { ChatInfo, ChatState } from '@/core/protocol';
import type { ChatDelivery } from '@/devices/hub';
import { ChatSession } from './session';
import type { Attachment } from './uploads';

const FEED = 'feed-1';
const HEAD = {
  connection: 'ready',
  running: false,
  turned: false,
  permissionMode: 'default',
  commands: [],
  capabilities: {},
  setup: {},
  permissions: [],
  tasks: [],
  runningSubagents: [],
  queued: [],
  earlierLost: false,
};
const row = (id: string, role: ChatMessage['role'], text: string, fields: Partial<ChatMessage> = {}): ChatMessage => ({
  id,
  role,
  parts: [{ kind: 'text', id: `${id}/text`, text }],
  ...fields,
});
const said = (id: string, text: string): ChatChange => ({ op: 'row', row: row(id, 'assistant', text) });
const headSet = (set: Record<string, unknown>): ChatChange => ({ op: 'head', set });
const asking = (requestId: string): PermissionAsk => ({
  requestId,
  sessionId: 's',
  toolCall: { toolCallId: `t-${requestId}`, title: 'rm -rf build', kind: 'execute', row: { action: 'execute', target: 'rm -rf build' } },
  options: [{ optionId: 'allow', name: 'Allow', kind: 'allow_once' }],
});

function live(rows: ChatMessage[], seq: number, head: Record<string, unknown> = {}) {
  return ChatAttachment.Live.new({
    viewJson: JSON.stringify({ head: { ...HEAD, ...head }, rows }),
    mark: { feed: FEED, seq: BigInt(seq) },
  });
}

const queued = (...messages: { id: string; text: string; paths: string[] }[]) =>
  headSet({ queued: messages.map((message) => ({ ...message, context: [] })) });

function fakeConnection(attachment: unknown) {
  return {
    isOpen: () => true,
    wakeChat: vi.fn(async () => {}),
    attachChat: vi.fn(async (_agent: string, _since?: { feed: string; seq: bigint }) => attachment),
    detachChat: vi.fn(async () => {}),
    attachFile: vi.fn(async (_agent: string, name: string, _mime: string, _bytes: ArrayBuffer) => `/host/pasted/${name}`),
    cancel: vi.fn(async () => {}),
    call: vi.fn(async (_method: string, _params: string) => 'null'),
    prompt: vi.fn(async (_agent: string, _messageId: string | undefined, _text: string, _paths: string[]) => {}),
    stopTask: vi.fn(async (_agent: string, _task: string) => {}),
    setPermissionMode: vi.fn(async (_agent: string, _mode: string) => {}),
    answerPermission: vi.fn(async () => {}),
    setChatConfig: vi.fn(async () => '{}'),
    chatHistory: vi.fn(),
  };
}

/** What the session asked the host to send, oldest first. */
const prompts = (connection: ReturnType<typeof fakeConnection>) =>
  connection.prompt.mock.calls.map(([agentId, messageId, text, paths]) => ({ agentId, messageId, text, paths }));

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
  const deliver = (...changes: ChatChange[]) =>
    take?.(changes.map((change) => ({ agentId: 'agent', seq: BigInt((seq += 1)), changesJson: JSON.stringify([change]) })));
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
  session.snapshot().view.messages.flatMap((message) => message.parts.flatMap((part) => (part.kind === 'text' ? [part.text] : [])));
const info = (state: ChatState, asleep = false) => ({ agentId: 'agent', state, asleep }) as ChatInfo;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('a chat session', () => {
  it('keeps changes that arrived but were not drawn when the chat is left', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 5));
    const release = session.hold(connection as unknown as ConnectionLike);
    await settle();
    startAt(5);
    deliver(headSet({ running: true }), said('m6', 'half'));
    expect(session.snapshot().view.running).toBe(false);

    release();
    expect(texts(session)).toEqual(['half']);
    expect(session.snapshot().view.running).toBe(true);

    const resumed = fakeConnection(
      ChatAttachment.Resumed.new({ changesJson: JSON.stringify([headSet({ running: false })]), mark: { feed: FEED, seq: 8n } }),
    );
    session.hold(resumed as unknown as ConnectionLike);
    await settle();
    expect(resumed.attachChat.mock.calls[0][1]).toEqual({ feed: FEED, seq: 7n });
    expect(session.snapshot().view.running).toBe(false);
    expect(texts(session)).toEqual(['half']);
    drawFrame();
  });

  it('moves the mark only once changes are drawn', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const first = fakeConnection(live([], 2));
    session.hold(first as unknown as ConnectionLike);
    await settle();
    startAt(2);
    deliver(said('m3', 'one'));
    expect(texts(session)).toEqual([]);
    drawFrame();
    expect(texts(session)).toEqual(['one']);

    session.hostSaw(info('stopped'), []);
    session.hostSaw(info('ready'), []);
    await settle();
    expect(first.attachChat.mock.calls[1][1]).toEqual({ feed: FEED, seq: 3n });
  });

  it('marks a message the host did not take, sends it again on Retry, and shows it once the host does', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 0));
    connection.prompt.mockRejectedValueOnce(MobileError.Connection.new({ message: 'the connection dropped' }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.send('hello');
    expect(session.snapshot().view.running).toBe(true);
    await settle();
    const [[id, unsent]] = [...session.snapshot().unsent];
    expect(unsent).toEqual({ state: 'failed', problem: 'the connection dropped' });
    expect(session.snapshot().view.running).toBe(false);
    expect(texts(session)).toEqual(['hello']);

    session.retrySend(id);
    expect(session.snapshot().unsent.get(id)?.state).toBe('sending');
    await settle();
    expect(session.snapshot().unsent.size).toBe(0);
    expect(prompts(connection).map((sent) => sent.messageId)).toEqual([id, id]);
    expect(texts(session)).toEqual(['hello']);

    startAt(0);
    deliver({ op: 'row', row: row('m1', 'user', 'hello', { promptId: id }) });
    drawFrame();
    expect(session.snapshot().view.messages.map((message) => message.id)).toEqual(['m1']);
  });

  it('keeps a message it could not send through a fresh view of the chat', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([row('m1', 'user', 'before')], 2));
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
    expect(session.snapshot().view.messages.at(-1)?.id).toBe(id);
  });

  it('shows only the permission requests the host still waits on', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 3, { running: true, permissions: [asking('old'), asking('open')] }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    const ids = () => session.snapshot().permissions.map((request) => request.requestId);
    expect(ids()).toEqual(['old', 'open']);

    session.hostSaw(info('ready'), ['open']);
    expect(ids()).toEqual(['open']);

    startAt(3);
    deliver(headSet({ permissions: [asking('old'), asking('open'), asking('new')] }));
    drawFrame();
    expect(ids()).toEqual(['open', 'new']);
    vi.setSystemTime(Date.now() + 3000);
    session.hostSaw(info('ready'), ['open', 'other']);
    expect(ids()).toEqual(['open']);
  });

  it('keeps a failed answer apart from the agent', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 2, { running: true, permissions: [asking('ask')] }));
    connection.answerPermission.mockRejectedValueOnce(new Error('timed out'));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.answer('ask', 'allow');
    session.answer('ask', 'allow');
    expect(session.snapshot().answering.has('ask')).toBe(true);
    await settle();
    expect(connection.answerPermission).toHaveBeenCalledTimes(1);
    const { view, notice, permissions, answering } = session.snapshot();
    expect(notice).toBe('The answer did not reach the host: timed out');
    expect(view.running).toBe(true);
    expect(view.error).toBeUndefined();
    expect(permissions).toHaveLength(1);
    expect(answering.size).toBe(0);
  });

  it("hands a message written mid-turn to the host's queue, and shows what waits there", async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 1, { running: true }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    await session.send('next');
    expect(session.snapshot().view.queued.map((held) => held.text)).toEqual(['next']);
    await settle();
    expect(prompts(connection)).toMatchObject([{ agentId: 'agent', text: 'next', paths: [] }]);
    expect(texts(session)).toEqual([]);

    startAt(1);
    deliver(queued({ id: 'q1', text: 'next', paths: [] }));
    drawFrame();
    expect(session.snapshot().view.queued.map((held) => held.text)).toEqual(['next']);
  });

  it('hands what waited back to the composer on Stop', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 1, { running: true }));
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
    const connection = fakeConnection(live([], 1, { running: true }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();

    session.steer(['q1']);
    session.dropQueued('q2');
    await settle();
    expect(connection.call).toHaveBeenCalledWith('steerQueued', JSON.stringify({ agentId: 'agent', ids: ['q1'] }));
    expect(connection.call).toHaveBeenCalledWith('dropQueued', JSON.stringify({ agentId: 'agent', ids: ['q2'] }));
  });

  it('keeps the permission mode it set until the host says otherwise', async () => {
    const { session, deliver, startAt, drawFrame } = harness();
    const connection = fakeConnection(live([], 0, { permissionMode: 'workspace-write' }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    expect(session.snapshot().permissionMode).toBe('workspace-write');

    session.setPermissionMode('bypass');
    expect(session.snapshot().changingMode).toBe(true);
    await settle();
    expect(connection.setPermissionMode).toHaveBeenCalledWith('agent', 'bypass');
    expect(session.snapshot()).toMatchObject({ permissionMode: 'bypass', changingMode: false });

    startAt(0);
    deliver(headSet({ running: true }));
    drawFrame();
    expect(session.snapshot().permissionMode).toBe('bypass');
    deliver(headSet({ permissionMode: 'default' }));
    drawFrame();
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

  it('shows the message with its files at once and sends their host paths once they are there', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('shot.jpg', 'image'), picked('notes.txt', 'file')]);

    expect(await session.send('look')).toBe(true);
    const shown = session.snapshot();
    expect(shown.attachments).toHaveLength(0);
    const message = shown.view.messages.at(-1);
    expect(message?.attachments).toEqual(['file:///picked/shot.jpg', 'file:///picked/notes.txt']);
    expect(shown.unsent.get(message?.id ?? '')?.state).toBe('sending');
    expect(shown.sentFiles.get('file:///picked/shot.jpg')?.name).toBe('shot.jpg');

    await settle();
    expect(connection.attachFile.mock.calls.map((call) => call[1])).toEqual(['shot.jpg', 'notes.txt']);
    expect(prompts(connection)).toMatchObject([{ text: 'look', paths: ['/host/pasted/shot.jpg', '/host/pasted/notes.txt'] }]);
    const { view, sentFiles, unsent } = session.snapshot();
    expect(unsent.size).toBe(0);
    expect(view.messages.at(-1)?.attachments).toEqual(['/host/pasted/shot.jpg', '/host/pasted/notes.txt']);
    expect(sentFiles.get('/host/pasted/shot.jpg')?.uri).toBe('file:///picked/shot.jpg');
  });

  it('marks the message not sent while a file did not reach the host, and sends only the rest again', async () => {
    const { session } = harness();
    const connection = fakeConnection(live([], 0));
    connection.attachFile.mockRejectedValueOnce(MobileError.Connection.new({ message: 'the connection dropped' }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('a.jpg', 'image'), picked('b.pdf', 'file')]);

    await session.send('both');
    await settle();
    const id = session.snapshot().view.messages.at(-1)?.id ?? '';
    expect(session.snapshot().unsent.get(id)).toEqual({ state: 'failed', problem: 'the connection dropped' });
    expect(prompts(connection)).toEqual([]);

    session.retrySend(id);
    await settle();
    expect(connection.attachFile).toHaveBeenCalledTimes(3);
    expect(prompts(connection)).toMatchObject([{ text: 'both', paths: ['/host/pasted/a.jpg', '/host/pasted/b.pdf'] }]);
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
    const connection = fakeConnection(live([], 1, { running: true }));
    session.hold(connection as unknown as ConnectionLike);
    await settle();
    session.attach([picked('shot.jpg', 'image')]);
    await session.send('next');
    expect(session.snapshot().view.queued.map((held) => held.text)).toEqual(['next']);
    await settle();
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
