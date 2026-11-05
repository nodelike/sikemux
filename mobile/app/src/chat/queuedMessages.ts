import { basename } from '@shared/lib/paths';
import type { QueuedMessage } from '@shared/chat/types';

export const queuedLabel = (message: QueuedMessage): string =>
  message.text || [...message.paths.map(basename), ...message.context.map((item) => item.title)].join(', ');
