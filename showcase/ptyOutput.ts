// The core's limits in src-tauri/crates/sikemux-core/src/server/session.rs.
const BATCH_BYTES = 64 * 1024;
const MAX_UNACKED_BYTES = 512 * 1024;

export interface PtyChannel {
  onmessage: (bytes: ArrayBuffer) => void;
}

/**
 * A terminal's output sent the way the core sends it: at most 64 KB a
 * message, each its own task, and nothing more while 512 KB wait for the
 * terminal to finish writing them.
 */
export class PtyOutput {
  private readonly pending: {
    bytes: Uint8Array;
    sent: (at: number) => void;
  }[] = [];
  private readonly waiting: { at: number; done: () => void }[] = [];
  private readonly messages = new MessageChannel();
  private unacked = 0;
  private sentBytes = 0;
  private ackedBytes = 0;

  constructor(channel: PtyChannel) {
    this.messages.port1.onmessage = ({ data }) => channel.onmessage(data);
  }

  /** Resolves with where the bytes end in the stream once all of them are sent, as a program's write returns once the terminal takes it. */
  write(bytes: Uint8Array, { ahead = false } = {}): Promise<number> {
    return new Promise((sent) => {
      if (ahead) this.pending.unshift({ bytes, sent });
      else this.pending.push({ bytes, sent });
      this.pump();
    });
  }

  /** Resolves once the terminal has written everything up to `at`. */
  acked(at: number): Promise<void> {
    if (at <= this.ackedBytes) return Promise.resolve();
    return new Promise((done) => this.waiting.push({ at, done }));
  }

  get sent(): number {
    return this.sentBytes;
  }

  ack(bytes: number): void {
    this.ackedBytes += bytes;
    this.unacked = Math.max(0, this.unacked - bytes);
    for (const waiter of this.waiting.filter(
      ({ at }) => at <= this.ackedBytes,
    )) {
      this.waiting.splice(this.waiting.indexOf(waiter), 1);
      waiter.done();
    }
    this.pump();
  }

  private pump(): void {
    while (this.unacked < MAX_UNACKED_BYTES && this.pending.length > 0) {
      const next = this.pending[0];
      const message = next.bytes.slice(0, BATCH_BYTES);
      next.bytes = next.bytes.subarray(message.length);
      this.unacked += message.length;
      this.sentBytes += message.length;
      this.messages.port2.postMessage(message.buffer, [message.buffer]);
      if (next.bytes.length > 0) continue;
      this.pending.shift();
      next.sent(this.sentBytes);
    }
  }
}
