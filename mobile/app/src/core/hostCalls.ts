import { useEffect, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import { problem } from '@/core/problem';

/** What the host last answered a call with, and why the latest read failed if it did. */
export type HostRead<T> = { data?: T; problem?: string };

/** What the host answers `method` with, read again whenever `seen` changes. */
export function useHostCall<T>(connection: ConnectionLike | undefined, method: string, params: unknown, seen: string): HostRead<T> {
  const [read, setRead] = useState<HostRead<T>>({});
  const asked = JSON.stringify(params);
  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call(method, asked).then(
      (text) => {
        if (current) setRead({ data: JSON.parse(text) as T });
      },
      (error: unknown) => {
        if (current) setRead((last) => ({ data: last.data, problem: problem(error) }));
      },
    );
    return () => {
      current = false;
    };
  }, [connection, method, asked, seen]);
  return read;
}
