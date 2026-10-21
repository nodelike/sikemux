import { Directory } from 'expo-file-system';

const RETRY_FAILED_MS = 60_000;

/** The folder under `root` where this phone keeps its copies of one host's files. */
export function hostFolder(root: Directory, name: string, core: string): Directory {
  return new Directory(root, name, core.slice(0, 16));
}

/** Where a `file://` URI points, as the Rust client writes to it. */
export function pathOf(uri: string): string {
  return decodeURIComponent(uri.replace(/^file:\/\//, ''));
}

/** When fetching each thing last failed, so it is asked for again only once a minute has passed. */
export function failures() {
  const failedAt = new Map<string, number>();
  return {
    recently: (key: string) => Date.now() - (failedAt.get(key) ?? -Infinity) < RETRY_FAILED_MS,
    record: (key: string) => void failedAt.set(key, Date.now()),
    clear: (key: string) => void failedAt.delete(key),
  };
}
