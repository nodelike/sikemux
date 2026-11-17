import { describe, expect, it } from 'vitest';
import { sentPrompts } from './promptHistory';
import type { ChatMessage } from '@shared/wire.generated';

const said = (role: ChatMessage['role'], text: string): ChatMessage => ({
  id: text,
  role,
  parts: [{ id: `${text}-t`, kind: 'text', text }],
});

describe('the messages ↑ can bring back', () => {
  it('are what was sent, oldest first, then anything still queued', () => {
    const messages = [said('user', 'fix the build'), said('assistant', 'done'), said('user', ' run the tests ')];
    expect(sentPrompts(messages, ['and push'])).toEqual(['fix the build', 'run the tests', 'and push']);
  });

  it('keep a message sent twice in a row once, and skip one that was only files', () => {
    const onlyFiles: ChatMessage = { id: 'f', role: 'user', parts: [], attachments: ['/a.png'] };
    expect(sentPrompts([said('user', 'again'), said('user', 'again'), onlyFiles])).toEqual(['again']);
  });

  it('read only the words of a message that also carried a picture', () => {
    const withPicture: ChatMessage = {
      id: 'p',
      role: 'user',
      parts: [
        { id: 'p-c', kind: 'content', content: { type: 'image', data: 'AAAA', mimeType: 'image/png' } },
        { id: 'p-t', kind: 'text', text: 'what is this' },
      ],
    };
    expect(sentPrompts([withPicture])).toEqual(['what is this']);
  });
});
