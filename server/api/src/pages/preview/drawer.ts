import { once } from "node:events";
import { Worker } from "node:worker_threads";

import type { Card } from "./card.ts";

const MOST_KEPT = 64;
const KEPT_MS = 10 * 60_000;
const MOST_WAITING = 8;
const MOST_MS = 15_000;
const IDLE_MS = 60_000;

/**
 * Draws share pictures on one thread of its own, one at a time, and keeps the latest for ten
 * minutes, over the burst of requests a posted link brings. The thread with the first picture asked for and ends once none has been for a minute.
 */
export class PictureDrawer {
  private readonly script: URL;
  private readonly kept = new Map<string, Buffer>();
  private readonly drawing = new Map<string, Promise<Buffer>>();
  private last: Promise<unknown> = Promise.resolve();
  private worker: Worker | null = null;
  private idle: NodeJS.Timeout | undefined;

  constructor(script: URL) {
    this.script = script;
  }

  /** The card's picture, or null when it has to be drawn and `may` refuses or too many wait. */
  async picture(card: Card, may: () => boolean): Promise<Buffer | null> {
    const key = JSON.stringify([card.id, card.title, card.sharedBy]);
    const known = this.kept.get(key) ?? this.drawing.get(key);
    if (known) return known;
    if (this.drawing.size >= MOST_WAITING || !may()) return null;
    const drawn = this.last
      .then(() => this.draw(card))
      .finally(() => this.drawing.delete(key));
    this.last = drawn.catch(() => {});
    this.drawing.set(key, drawn);
    const png = await drawn;
    this.kept.set(key, png);
    setTimeout(() => this.kept.delete(key), KEPT_MS).unref();
    for (const oldest of this.kept.keys()) {
      if (this.kept.size <= MOST_KEPT) break;
      this.kept.delete(oldest);
    }
    return png;
  }

  private async draw(card: Card): Promise<Buffer> {
    clearTimeout(this.idle);
    const worker = (this.worker ??= this.start());
    try {
      worker.postMessage(card);
      const [png] = (await once(worker, "message", {
        signal: AbortSignal.timeout(MOST_MS),
      })) as [Uint8Array];
      this.idle = setTimeout(() => this.end(), IDLE_MS).unref();
      return Buffer.from(png);
    } catch (error) {
      this.end();
      throw error;
    }
  }

  private start(): Worker {
    const worker = new Worker(this.script, {
      resourceLimits: { maxOldGenerationSizeMb: 256 },
    });
    worker.unref();
    worker.on("error", () => {
      if (this.worker === worker) this.worker = null;
    });
    return worker;
  }

  private end() {
    void this.worker?.terminate();
    this.worker = null;
  }
}
