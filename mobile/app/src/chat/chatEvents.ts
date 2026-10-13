import { chatActions } from '@mac/chat/acpEvents';
import type { ChatAction } from '@mac/chat/types';

import type { CoreChatEvent } from '@/core/protocol';

/** A run of the host's chat events, as the reducer takes them. */
export function parsed(json: string): ChatAction[] {
  return (JSON.parse(json) as CoreChatEvent[]).flatMap(chatActions);
}
