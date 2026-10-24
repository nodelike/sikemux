import { DemoScreen } from "./terminalFrames";

// The core's pacing, in src-tauri/crates/sikemux-core/src/server/session.rs.
const FRAME_GAP_MS = 8;
const MAX_FRAMES_IN_FLIGHT = 2;

export interface PtyChannel {
  onmessage: (bytes: ArrayBuffer) => void;
}

/**
 * A terminal's frames sent the way the core sends them: the first change at
 * once, then at most one frame per 8 ms, and none while two wait to be drawn.
 */
export class PtyOutput {
  readonly screen = new DemoScreen();
  private inFlight = 0;
  private lastSent = 0;
  private dirty = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly drawn: (() => void)[] = [];
  private framesSent = 0;
  /** Runs before each frame is made, so a flood can add what came in meanwhile. */
  beforeFrame: (() => void) | null = null;

  constructor(private readonly channel: PtyChannel) {}

  get frames(): number {
    return this.framesSent;
  }

  write(text: string): void {
    this.screen.write(text);
    this.changed();
  }

  /** Marks the screen changed, as a flood that writes on every frame does. */
  changed(): void {
    this.dirty = true;
    this.pump();
  }

  /** Resolves once a frame sent after this call has been drawn. */
  drawnSoon(): Promise<void> {
    return new Promise((done) => this.drawn.push(done));
  }

  ack(): void {
    this.inFlight = 0;
    for (const done of this.drawn.splice(0)) done();
    this.pump();
  }

  private pump(): void {
    if (!this.dirty || this.inFlight >= MAX_FRAMES_IN_FLIGHT || this.timer)
      return;
    const wait = this.lastSent + FRAME_GAP_MS - performance.now();
    if (wait > 0) {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.pump();
      }, wait);
      return;
    }
    this.beforeFrame?.();
    this.dirty = this.beforeFrame !== null;
    const frame = this.screen.frame();
    this.inFlight++;
    this.framesSent++;
    this.lastSent = performance.now();
    this.channel.onmessage(frame.buffer as ArrayBuffer);
  }
}
