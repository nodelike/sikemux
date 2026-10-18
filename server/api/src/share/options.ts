export const SHARE_PATH = "/v1/share";

/** Every limit and timer of share connections, so tests can shorten them. */
export interface ShareOptions {
  challengeMs: number;
  helloTimeoutMs: number;
  heartbeatMs: number;
  silenceMs: number;
  /** How long a viewer's connection may outlive its token's expiry before it is closed. */
  authGraceMs: number;
  maxSockets: number;
  upgradesPerMinute: number;
  failedHellosPerMinute: number;
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
  challengeMs: 30_000,
  helloTimeoutMs: 10_000,
  heartbeatMs: 25_000,
  silenceMs: 60_000,
  authGraceMs: 30_000,
  maxSockets: 20_000,
  upgradesPerMinute: 60,
  failedHellosPerMinute: 10,
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
  reconnectSpreadMs: 15_000,
};
