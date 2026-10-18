import type {
  ShareEndReason,
  ShareFrame,
  ShareServerMessage,
} from "@sikemux/protocol";

import type { ShareOptions } from "./options.ts";

/** What the relay needs of a host's connection. */
export interface HostLink {
  readonly key: string;
  send(message: ShareServerMessage): void;
}

/** What the relay needs of a viewer's connection. */
export interface ViewerLink {
  readonly id: string;
  send(message: ShareServerMessage): void;
  /** A message already written out, sent as it is to every viewer it is for. */
  sendText(text: string): void;
  /** The share ended: say so and close. */
  end(reason: ShareEndReason): void;
}

/** One open share: its frames since the last snapshot, its host when connected, and its viewers. */
export class Room {
  readonly id: string;
  readonly hostKey: string;
  readonly expiresAt: Date;
  host: HostLink | null = null;
  readonly viewers = new Map<string, ViewerLink>();
  private frames: ShareFrame[] = [];
  private bytes = 0;
  /** The frames were let go of; nothing is kept again until a snapshot comes. */
  private waitingForSnapshot = false;
  private expiry: NodeJS.Timeout | undefined;

  constructor(
    id: string,
    hostKey: string,
    expiresAt: Date,
    onExpired: (room: Room) => void,
  ) {
    this.id = id;
    this.hostKey = hostKey;
    this.expiresAt = expiresAt;
    const wait = Math.max(0, expiresAt.getTime() - Date.now());
    this.expiry = setTimeout(
      () => onExpired(this),
      Math.min(wait, 2 ** 31 - 1),
    );
  }

  get lastSeq(): number {
    return this.frames.at(-1)?.seq ?? 0;
  }

  /** Keeps `frame` for later viewers and hands it to those watching now. */
  publish(frame: ShareFrame, options: ShareOptions) {
    const size = frame.sealed.length;
    if (frame.snapshot) {
      this.frames = [frame];
      this.bytes = size;
      this.waitingForSnapshot = false;
    } else if (!this.waitingForSnapshot) {
      if (this.bytes + size > options.backlogBytes) {
        this.frames = [];
        this.bytes = 0;
        this.waitingForSnapshot = true;
        this.host?.send({ type: "snapshot", share: this.id });
      } else {
        this.frames.push(frame);
        this.bytes += size;
      }
    }
    this.toViewers({ type: "frames", frames: [frame] });
  }

  /** Writes `message` out once, however many watch: a snapshot can run to megabytes. */
  private toViewers(message: ShareServerMessage) {
    if (this.viewers.size === 0) return;
    const text = JSON.stringify(message);
    for (const viewer of this.viewers.values()) viewer.sendText(text);
  }

  /** The frames a viewer holding everything through `since` still needs: the rest, or all from the snapshot. */
  framesAfter(since: number | undefined): ShareFrame[] {
    const first = this.frames[0]?.seq;
    if (since === undefined || first === undefined) return this.frames;
    if (since < first || since > this.lastSeq) return this.frames;
    return this.frames.filter((frame) => frame.seq > since);
  }

  /** A host sends a snapshot after every open, so it is asked for one only when frames were let go of since. */
  attachHost(host: HostLink) {
    this.host = host;
    host.send({ type: "viewers", share: this.id, count: this.viewers.size });
    if (this.waitingForSnapshot)
      host.send({ type: "snapshot", share: this.id });
    this.toViewers({ type: "host", online: true });
  }

  detachHost(host: HostLink) {
    if (this.host !== host) return;
    this.host = null;
    this.toViewers({ type: "host", online: false });
  }

  countViewers() {
    this.host?.send({
      type: "viewers",
      share: this.id,
      count: this.viewers.size,
    });
  }

  end(reason: ShareEndReason) {
    clearTimeout(this.expiry);
    this.host?.send({ type: "ended", share: this.id, reason });
    for (const viewer of this.viewers.values()) viewer.end(reason);
    this.viewers.clear();
    this.frames = [];
    this.bytes = 0;
  }
}

/** Every open share this process holds, by id, and each host's connection, by key. */
export class Relay {
  readonly rooms = new Map<string, Room>();
  readonly hosts = new Map<string, HostLink>();
  sockets = 0;
}
