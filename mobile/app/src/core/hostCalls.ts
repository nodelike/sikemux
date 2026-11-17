import { useEffect, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import { problem } from '@/core/problem';
import type { CoreCalls } from '@shared/wire.generated';

/** A method this phone calls, as `mobile/app/wire.json` names them. */
type HostMethod = keyof CoreCalls;

/** What the host last answered a call with, and why the latest read failed if it did. */
type HostRead<T> = { data?: T; problem?: string };

export async function callHost<M extends HostMethod>(
  connection: ConnectionLike,
  method: M,
  params: CoreCalls[M]['params'],
): Promise<CoreCalls[M]['answer']> {
  return JSON.parse(await connection.call(method, JSON.stringify(params))) as CoreCalls[M]['answer'];
}

/** What the host answers `method` with, read again whenever `seen` changes. */
export function useHostCall<M extends HostMethod>(
  connection: ConnectionLike | undefined,
  method: M,
  params: CoreCalls[M]['params'],
  seen: string,
): HostRead<CoreCalls[M]['answer']> {
  const [read, setRead] = useState<HostRead<CoreCalls[M]['answer']>>({});
  const asked = JSON.stringify(params);
  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call(method, asked).then(
      (text) => {
        if (current) setRead({ data: JSON.parse(text) as CoreCalls[M]['answer'] });
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
