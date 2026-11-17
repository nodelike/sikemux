import { requireOptionalNativeModule } from 'expo';

/** A file another app shared; one too large to stash has no `uri`. */
export type SharedFile = { uri: string | null; name: string; mime: string; size: number };

export type Shared = { text?: string | null; subject?: string | null; files: SharedFile[] };

type StashModule = {
  /** What the app was opened to share, once; null when it was opened otherwise. */
  take?(): Promise<Shared | null>;
  /** Goes back to the app that shared. */
  leave?(): void;
  addListener?(event: 'onShare', listener: () => void): { remove(): void };
};

export const stashNative = requireOptionalNativeModule<StashModule>('SikemuxStash');
