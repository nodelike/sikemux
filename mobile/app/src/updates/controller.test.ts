import { beforeEach, describe, expect, it, vi } from 'vitest';

import { AWAY_BEFORE_RESTART_MS, CHECK_EVERY_MS, UpdateController, type UpdateDeps } from './controller';

let now: number;
let deps: UpdateDeps & {
  check: ReturnType<typeof vi.fn<UpdateDeps['check']>>;
  fetch: ReturnType<typeof vi.fn<UpdateDeps['fetch']>>;
  reload: ReturnType<typeof vi.fn<UpdateDeps['reload']>>;
};

beforeEach(() => {
  now = 1_000_000;
  deps = {
    check: vi.fn(async () => ({ isAvailable: true })),
    fetch: vi.fn(async () => ({ isNew: true })),
    reload: vi.fn(async () => {}),
    now: () => now,
  };
});

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('UpdateController', () => {
  it('checks and fetches on coming to the front, at most every five minutes', async () => {
    deps.check.mockResolvedValue({ isAvailable: false });
    const updates = new UpdateController(deps);
    updates.foreground();
    await settle();
    now += CHECK_EVERY_MS - 1;
    updates.foreground();
    await settle();
    expect(deps.check).toHaveBeenCalledTimes(1);
    now += 1;
    updates.foreground();
    await settle();
    expect(deps.check).toHaveBeenCalledTimes(2);
    expect(deps.fetch).not.toHaveBeenCalled();
  });

  it('keeps what is on screen after a glance away, and restarts into the update after a longer one', async () => {
    const updates = new UpdateController(deps);
    updates.foreground();
    await settle();
    expect(deps.fetch).toHaveBeenCalledTimes(1);
    updates.background();
    now += AWAY_BEFORE_RESTART_MS - 1;
    updates.foreground();
    expect(deps.reload).not.toHaveBeenCalled();
    updates.background();
    now += AWAY_BEFORE_RESTART_MS;
    updates.foreground();
    expect(deps.reload).toHaveBeenCalledTimes(1);
  });

  it('restarts into an update fetched as the app started, without asking again', async () => {
    const updates = new UpdateController(deps);
    updates.downloaded();
    updates.foreground();
    await settle();
    expect(deps.check).not.toHaveBeenCalled();
    updates.background();
    now += AWAY_BEFORE_RESTART_MS;
    updates.foreground();
    expect(deps.reload).toHaveBeenCalledTimes(1);
  });

  it('survives a failed check and tries again after the wait', async () => {
    deps.check.mockRejectedValueOnce(new Error('offline'));
    const updates = new UpdateController(deps);
    updates.foreground();
    await settle();
    updates.background();
    now += AWAY_BEFORE_RESTART_MS;
    updates.foreground();
    expect(deps.reload).not.toHaveBeenCalled();
    now += CHECK_EVERY_MS;
    updates.foreground();
    await settle();
    expect(deps.fetch).toHaveBeenCalledTimes(1);
  });

  it('does not count an update the phone already has as waiting', async () => {
    deps.fetch.mockResolvedValue({ isNew: false });
    const updates = new UpdateController(deps);
    updates.foreground();
    await settle();
    updates.background();
    now += AWAY_BEFORE_RESTART_MS;
    updates.foreground();
    expect(deps.reload).not.toHaveBeenCalled();
  });

  it('reports a restart that fails instead of throwing', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    deps.reload.mockRejectedValue(new Error('no'));
    await new UpdateController(deps).restart();
    expect(warn).toHaveBeenCalled();
  });
});
