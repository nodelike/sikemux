import { useEffect, useState } from 'react';
import type { ConnectionLike } from '@sikemux/native';

import { problem } from '@/core/problem';
import type { CoreCalls } from '@shared/wire.generated';

/** What the core takes for `method`: its declared params, or anything for a call that names none. */
type CallParams<M extends string> = M extends keyof CoreCalls ? CoreCalls[M]['params'] : unknown;
/** What the core answers `method` with: its declared answer, or unknown for a call that names none. */
type CallAnswer<M extends string> = M extends keyof CoreCalls ? CoreCalls[M]['answer'] : unknown;

/** What the host last answered a call with, and why the latest read failed if it did. */
type HostRead<T> = { data?: T; problem?: string };

/** Calls the core's `method` on the host. */
export async function callHost<M extends string>(connection: ConnectionLike, method: M, params: CallParams<M>): Promise<CallAnswer<M>> {
  return JSON.parse(await connection.call(method, JSON.stringify(params))) as CallAnswer<M>;
}

/** What the host answers `method` with, read again whenever `seen` changes. */
export function useHostCall<M extends string>(
  connection: ConnectionLike | undefined,
  method: M,
  params: CallParams<M>,
  seen: string,
): HostRead<CallAnswer<M>> {
  const [read, setRead] = useState<HostRead<CallAnswer<M>>>({});
  const asked = JSON.stringify(params);
  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call(method, asked).then(
      (text) => {
        if (current) setRead({ data: JSON.parse(text) as CallAnswer<M> });
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
