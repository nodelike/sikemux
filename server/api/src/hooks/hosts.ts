export interface HookHost {
  readonly userId: string;
  readonly key: string;
  /** Look for requests to send. */
  wake(): void;
  /** Close if the host has no webhooks and nothing waiting. */
  checkIdle(): void;
  replaced(): void;
  removed(): void;
  bye(reconnectAfterMs: number): void;
}

/** The connected hosts, one connection each, so a request that arrives reaches its host at once. */
export class HookHosts {
  private readonly byKey = new Map<string, HookHost>();

  get open(): number {
    return this.byKey.size;
  }

  admit(host: HookHost) {
    const older = this.byKey.get(host.key);
    this.byKey.set(host.key, host);
    older?.replaced();
  }

  remove(host: HookHost) {
    if (this.byKey.get(host.key) === host) this.byKey.delete(host.key);
  }

  wake(key: string) {
    this.byKey.get(key)?.wake();
  }

  checkIdle(key: string) {
    this.byKey.get(key)?.checkIdle();
  }

  /** The connections of `userId`'s hosts, or of every host when it is undefined. */
  of(userId?: string): HookHost[] {
    return [...this.byKey.values()].filter(
      (host) => userId === undefined || host.userId === userId,
    );
  }
}
