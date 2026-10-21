import { createContext, useContext, useEffect, useState } from 'react';
import { File, Paths } from 'expo-file-system';

import { connectionOf, useLive } from '@/devices/hub';
import { failures, hostFolder, pathOf } from '@/devices/savedFiles';

/** The host whose chat is on the screen, for fetching the files its messages carried. */
export const HostContext = createContext('');

/** A shared chat's pictures, from the computer that shared it, as data URIs. */
export const SharedFilesContext = createContext<((path: string) => Promise<string | null>) | null>(null);

/** Fetches in flight, so a file shown twice is asked for once. */
const fetching = new Map<string, Promise<string>>();
/** Rows are drawn again as the transcript scrolls, and asking for a file that just failed each time would not help. */
const failed = failures();

function saved(core: string, path: string): File {
  return new File(hostFolder(Paths.cache, 'chat-files', core), path.slice(path.lastIndexOf('/') + 1));
}

/** A file on the host, copied to this phone once and answered with where it is here. */
export function useHostFile(path: string, wanted: boolean): string | undefined {
  const shared = useContext(SharedFilesContext);
  const fromShare = useSharedFile(shared, path, wanted);
  const fromHost = useFileOnHost(shared ? '' : path, wanted && !shared);
  return shared ? fromShare : fromHost;
}

function useSharedFile(read: ((path: string) => Promise<string | null>) | null, path: string, wanted: boolean): string | undefined {
  const [uri, setUri] = useState<string>();
  useEffect(() => {
    if (!read || !wanted || uri) return;
    let current = true;
    void read(path).then((found) => {
      if (current && found) setUri(found);
    });
    return () => {
      current = false;
    };
  }, [read, path, wanted, uri]);
  return uri;
}

function useFileOnHost(path: string, wanted: boolean): string | undefined {
  const core = useContext(HostContext);
  const live = useLive(core);
  const connection = connectionOf(live);
  const key = `${core}/${path}`;
  const [uri, setUri] = useState(() => {
    const file = core ? saved(core, path) : undefined;
    return file?.exists ? file.uri : undefined;
  });

  useEffect(() => {
    if (!wanted || uri || !core || !connection) return;
    if (failed.recently(key)) return;
    let current = true;
    let fetch = fetching.get(key);
    if (!fetch) {
      const file = saved(core, path);
      // The Rust client decodes and writes the file, so its bytes never pass through JavaScript.
      fetch = connection
        .callToFile('chatFile', JSON.stringify({ path }), 'data', pathOf(file.uri))
        .then(() => file.uri)
        .finally(() => fetching.delete(key));
      fetching.set(key, fetch);
    }
    fetch.then(
      (found) => {
        failed.clear(key);
        if (current) setUri(found);
      },
      () => failed.record(key),
    );
    return () => {
      current = false;
    };
  }, [wanted, uri, core, connection, key, path]);

  return uri;
}
