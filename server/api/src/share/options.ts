import { SOCKET_DEFAULTS, type SocketOptions } from "../sockets/socket.ts";

export const SHARE_PATH = "/v1/share";

/** Every limit and timer of share connections, so tests can shorten them. */
export interface ShareOptions extends SocketOptions {
  maxSockets: number;
  hostMessagesPerSecond: number;
  viewerMessagesPerSecond: number;
  /** The largest message a host may send; a frame holds a whole snapshot of a chat's end. */
  maxHostMessageBytes: number;
  maxViewerMessageBytes: number;
  /** Shares one account may have open at once. */
  openPerUser: number;
  viewersPerShare: number;
  /** Past this the frames kept for late viewers are dropped and the host asked for a snapshot. */
  backlogBytes: number;
  /** The longest a share may stay open. */
  maxLifetimeMs: number;
  /** Above this many unsent bytes a viewer is closed, to come back from the frames it holds. */
  dropBytes: number;
  framesPerMessage: number;
  reconnectSpreadMs: number;
}

export const SHARE_OPTIONS: ShareOptions = {
  ...SOCKET_DEFAULTS,
  hostMessagesPerSecond: 400,
  viewerMessagesPerSecond: 20,
  maxHostMessageBytes: 6 * 1024 * 1024,
  maxViewerMessageBytes: 8 * 1024,
  openPerUser: 20,
  viewersPerShare: 100,
  backlogBytes: 8 * 1024 * 1024,
  maxLifetimeMs: 7 * 24 * 60 * 60_000,
  dropBytes: 16 * 1024 * 1024,
  framesPerMessage: 64,
};
