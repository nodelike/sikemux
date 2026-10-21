import { reconnectDelay } from '@protocol/live';

import { problem } from '@/core/problem';
import { AccountDeleting, ApiProblem } from './api';

/** How far signing this phone in to the account has got, so a screen can say why it stopped. */
export type AccountStatus =
  | { step: 'registering' }
  | { step: 'registered' }
  | { step: 'failed'; problem: string; retrying: boolean }
  | { step: 'deleting'; deletesAt: string };

export type SequenceDeps = {
  /** Adds this phone to the account; doing it again only renames it. */
  register(): Promise<void>;
  syncPush(): Promise<void>;
  live: { start(): void; stop(): void; nudge(): void };
  /** The account may be reachable again: read the hosts again if the last read failed. */
  hostsStale(): void;
  status(next: AccountStatus): void;
  random?: () => number;
};

const TOO_MANY = 429;

/** Out of reach, failing on its side or busy: worth trying again by itself. Anything else waits for the person. */
function retryable(error: unknown): boolean {
  return !(error instanceof ApiProblem) || error.unreachable || error.status === TOO_MANY;
}

/**
 * Signing in, in order: add this phone to the account, then send its notification token, then open the
 * live connection, which the server only takes from a phone on the account. A failed registration tries
 * again with backoff while the app is in front, and at once when it comes back to the front or the
 * phone's network returns.
 */
export class AccountSequence {
  private registered = false;
  private registering = false;
  private front = false;
  private ended = false;
  private attempt = 0;
  private retrying?: ReturnType<typeof setTimeout>;

  constructor(private readonly deps: SequenceDeps) {}

  start(front: boolean) {
    this.front = front;
    this.register();
  }

  foreground() {
    this.front = true;
    if (this.registered) {
      this.deps.live.start();
      this.push();
    } else this.retryNow();
  }

  background() {
    this.front = false;
    clearTimeout(this.retrying);
    this.deps.live.stop();
  }

  online() {
    if (!this.registered) return this.retryNow();
    this.deps.live.nudge();
    this.deps.hostsStale();
  }

  /** The system gave the phone a new notification token. */
  pushChanged() {
    if (this.registered) this.push();
  }

  retryNow() {
    if (this.registered || this.registering) return;
    this.attempt = 0;
    this.register();
  }

  stop() {
    this.ended = true;
    clearTimeout(this.retrying);
    this.deps.live.stop();
  }

  private register() {
    if (this.ended || this.registering) return;
    clearTimeout(this.retrying);
    this.registering = true;
    this.deps.status({ step: 'registering' });
    this.deps.register().then(
      () => {
        this.registering = false;
        if (this.ended) return;
        this.registered = true;
        this.deps.status({ step: 'registered' });
        this.push();
        if (this.front) this.deps.live.start();
      },
      (error: unknown) => {
        this.registering = false;
        if (this.ended) return;
        if (error instanceof AccountDeleting) {
          this.deps.status({ step: 'deleting', deletesAt: error.deletesAt });
          return;
        }
        console.warn('sikemux: could not add this phone to the account', error);
        const retrying = retryable(error);
        this.deps.status({ step: 'failed', problem: problem(error), retrying });
        if (!retrying || !this.front) return;
        this.retrying = setTimeout(() => this.register(), reconnectDelay(this.attempt, (this.deps.random ?? Math.random)()));
        this.attempt += 1;
      },
    );
  }

  private push() {
    this.deps.syncPush().catch((error: unknown) => console.warn('sikemux: could not register for notifications', error));
  }
}
