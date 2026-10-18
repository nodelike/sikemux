/** Asking the update server at most this often keeps a phone that flicks in and out of the app quiet. */
export const CHECK_EVERY_MS = 5 * 60 * 1000;
/** A return from a glance away keeps what is on screen; a longer absence restarts into a waiting update. */
export const AWAY_BEFORE_RESTART_MS = 60 * 1000;

export type UpdateDeps = {
  check(): Promise<{ isAvailable: boolean }>;
  fetch(): Promise<{ isNew: boolean }>;
  reload(): Promise<void>;
  now(): number;
};

/**
 * Over-the-air updates while the app runs: checks and fetches when it comes to the front and while it stays
 * there, at most every five minutes, and starts the fetched update the next time the person comes back after a while away.
 */
export class UpdateController {
  private lastCheck = -Infinity;
  private checking = false;
  private pending = false;
  private leftAt?: number;

  constructor(private readonly deps: UpdateDeps) {}

  /** An update was fetched, here or as the app started, and waits for a restart. */
  downloaded() {
    this.pending = true;
  }

  foreground() {
    const leftAt = this.leftAt;
    this.leftAt = undefined;
    if (this.pending && leftAt !== undefined && this.deps.now() - leftAt >= AWAY_BEFORE_RESTART_MS) {
      void this.restart();
      return;
    }
    void this.check();
  }

  background() {
    this.leftAt = this.deps.now();
  }

  async restart() {
    try {
      await this.deps.reload();
    } catch (error) {
      console.warn('sikemux: could not restart into the update', error);
    }
  }

  async check() {
    if (this.checking || this.pending || this.deps.now() - this.lastCheck < CHECK_EVERY_MS) return;
    this.checking = true;
    this.lastCheck = this.deps.now();
    try {
      const { isAvailable } = await this.deps.check();
      if (!isAvailable) return;
      const { isNew } = await this.deps.fetch();
      if (isNew) this.pending = true;
    } catch {
      // useOverTheAirUpdates reports failed checks and downloads, as expo-updates records them.
    } finally {
      this.checking = false;
    }
  }
}
