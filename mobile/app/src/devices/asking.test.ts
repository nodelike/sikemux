import { describe, expect, it } from 'vitest';

import { asking } from './asking';

const request = (toolCall: unknown) => ({ request: { requestId: 'r', sessionId: 's', toolCall, options: [] } });

describe('asking', () => {
  it('names the command an agent wants to run', () => {
    expect(asking(request({ toolCallId: 't', title: 'pnpm test', kind: 'execute' }))).toEqual({ verb: 'run', target: 'pnpm test' });
  });

  it('names the file an agent wants to edit by its name', () => {
    expect(asking(request({ toolCallId: 't', title: '/Users/me/proj/src/app.ts', kind: 'edit' }))).toEqual({
      verb: 'edit',
      target: 'app.ts',
    });
  });

  it('says use for a tool without a plain verb', () => {
    expect(asking(request({ toolCallId: 't', title: 'mcp__github__create_issue' }))?.verb).toBe('use');
  });

  it('gives nothing for a request it cannot read', () => {
    expect(asking({})).toBeNull();
    expect(asking(request(undefined))).toBeNull();
    expect(asking(request({ toolCallId: 't', title: '' }))).toBeNull();
  });
});
