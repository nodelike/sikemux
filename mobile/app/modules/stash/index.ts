import { requireOptionalNativeModule } from 'expo';

/** A file another app shared; one too large to stash has no `uri`. */
export type SharedFile = { uri: string | null; name: string; mime: string; size: number };

export type Shared = { text?: string | null; subject?: string | null; files: SharedFile[] };

type StashModule = {
  /** Android: what the app was opened to share, once; null when it was opened otherwise. */
  take?(): Promise<Shared | null>;
  /** Android: goes back to the app that shared. */
  leave?(): void;
  /** iOS: the share extension's chats and where the accounts server is, or null to sign it out. */
  setShared?(json: string | null): void;
  /** iOS: this phone's key, which the share extension signs with, or null to take it away. */
  setDeviceKey?(hex: string | null): void;
  addListener?(event: 'onShare', listener: () => void): { remove(): void };
};

export const stashNative = requireOptionalNativeModule<StashModule>('SikemuxStash');
