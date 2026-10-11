import { permissionRequest, promptAction, recordOf, statusFromEvent } from '@mac/chat/acpEvents';
import type { ChatAction } from '@mac/chat/types';

import type { CoreChatEvent } from '@/core/protocol';

/** The same mapping the Mac's useAcpSession does from a core chat event to the reducer. */
export function actions(event: CoreChatEvent): ChatAction[] {
  const payload = event.payload;
  switch (event.kind) {
    case 'status':
      return [{ type: 'status', state: statusFromEvent({ payload } as never) }];
    case 'ready':
      return [{ type: 'ready', capabilities: recordOf(payload.capabilities) ?? {}, setup: recordOf(payload.setup) ?? {} }];
    case 'session_update': {
      const rows = Array.isArray(payload.updates) ? payload.updates : [payload];
      return rows.flatMap((entry): ChatAction[] => {
        const row = recordOf(entry);
        const update = row && recordOf(row.update);
        return update && typeof row.sessionId === 'string' ? [{ type: 'session_update', sessionId: row.sessionId, update }] : [];
      });
    }
    case 'prompt': {
      const prompted = promptAction(payload);
      return prompted ? [prompted] : [];
    }
    case 'turn_started':
      return [{ type: 'turn_started' }];
    case 'turn_completed':
      return [
        {
          type: 'turn_completed',
          stopReason: typeof payload.stopReason === 'string' ? payload.stopReason : undefined,
          at: typeof payload.at === 'number' ? payload.at : undefined,
        },
      ];
    case 'permission_request': {
      const request = permissionRequest(payload);
      return request ? [{ type: 'permission_requested', request }] : [];
    }
    case 'error':
      return [{ type: 'error', message: typeof payload.message === 'string' ? payload.message : 'The agent stopped.' }];
    default:
      return [];
  }
}

export function parsed(json: string): ChatAction[] {
  return (JSON.parse(json) as CoreChatEvent[]).flatMap(actions);
}
