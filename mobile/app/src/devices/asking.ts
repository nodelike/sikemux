import { recordOf } from '@mac/chat/acpEvents';
import { toolKind, toolTarget } from '@mac/chat/toolLabels';

import type { Attention } from '@/core/protocol';

const SAID = new Set(['run', 'read', 'edit', 'delete', 'move', 'search', 'fetch']);

/** What a waiting agent asks to do, as "run" and "pnpm test"; null when its request names nothing. */
export function asking(attention: Pick<Attention, 'request'>): { verb: string; target: string } | null {
  const call = recordOf(attention.request?.toolCall);
  if (!call || typeof call.title !== 'string' || !call.title.trim()) return null;
  const tool = { ...call, toolCallId: String(call.toolCallId ?? ''), title: call.title };
  const target = toolTarget(tool);
  if (!target) return null;
  const kind = toolKind(tool);
  return { verb: SAID.has(kind) ? kind : 'use', target };
}
