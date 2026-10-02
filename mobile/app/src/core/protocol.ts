import { BuildChannel as NativeChannel, type DeviceView } from '@sikemux/native';

/** The core's protocol as the Rust client hands it over: typed records built from the core's own types. */
export type {
  Attention,
  Backdrop,
  ChatAttachment,
  ChatInfo,
  ChatMark,
  LauncherInfo,
  ProjectInfo,
  SessionInfo,
  Workspace,
} from '@sikemux/native';
export { ChatState, SessionKind } from '@sikemux/native';

/** Everything the phone shows of one Mac, as it last sent it. */
export type Snapshot = DeviceView;

/** Kept with the paired Mac, so it is a name rather than the native enum's number. */
export type BuildChannel = 'dev' | 'nightly' | 'stable';

export function channelName(channel: NativeChannel): BuildChannel {
  if (channel === NativeChannel.Dev) return 'dev';
  if (channel === NativeChannel.Nightly) return 'nightly';
  return 'stable';
}

/** A chat event as the Mac app's chat code reads it. */
export type CoreChatEvent = { kind: string; payload: Record<string, unknown> };
