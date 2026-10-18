import { describe, expect, it } from 'vitest';

import { keepUnchanged, readView } from '../src/core/protocol';

// What the phone's native layer hands over from a v0.5.2 host, the latest stable release, which sends no inbox.
const STABLE_HOST_VIEW = JSON.stringify({
  workspace: { projects: [], launchers: [], palette: {}, backdrop: { texture: true, image: null } },
  sessions: [],
  chats: [],
  attentions: [],
  recent: [],
  inbox: null,
});

describe('a view from the latest stable host', () => {
  it('reads with everything the screens use', () => {
    const view = readView(STABLE_HOST_VIEW);
    expect(view.inbox.items).toEqual([]);
    expect(view.inbox.working).toBe(0);
  });

  it('takes the next view the host sends', () => {
    const first = readView(STABLE_HOST_VIEW);
    expect(keepUnchanged(first, readView(STABLE_HOST_VIEW)).inbox).toBe(first.inbox);
  });
});
