import { formatBytes } from '@mac/lib/format';

import { MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS } from '@/core/wire.generated';

/** A photo or file picked on this phone for the next message. */
export type Attachment = {
  id: string;
  kind: 'image' | 'file';
  name: string;
  mime: string;
  /** Where it is on this phone. */
  uri: string;
  size: number;
  /** Where the host keeps it, once it is there. */
  path?: string;
  upload?: 'sending' | 'failed';
  problem?: string;
};

/** What of `picked` the host would take beside the `held` files already picked, and why the rest were left out. */
export function fitting(held: number, picked: readonly Attachment[]): { fits: Attachment[]; notice: string | null } {
  const tooBig = picked.filter((attachment) => attachment.size > MAX_ATTACHMENT_BYTES);
  const small = picked.filter((attachment) => attachment.size <= MAX_ATTACHMENT_BYTES);
  const room = Math.max(0, MAX_ATTACHMENTS - held);
  const fits = small.slice(0, room);
  if (tooBig.length) {
    const names = tooBig.map((attachment) => attachment.name).join(', ');
    return { fits, notice: `${names} ${tooBig.length === 1 ? 'is' : 'are'} over ${formatBytes(MAX_ATTACHMENT_BYTES)}, too big to send` };
  }
  return { fits, notice: small.length > room ? `a message takes at most ${MAX_ATTACHMENTS} files` : null };
}

/** Where the host keeps each file that got there. */
export function pathsOf(attachments: readonly Attachment[]): string[] {
  return attachments.flatMap((attachment) => (attachment.path ? [attachment.path] : []));
}

/**
 * Puts each file the host does not have yet on it, one at a time, marking each as it goes. Answers them all
 * once every one got there, or nothing when one did not.
 */
export async function uploadEach(
  attachments: readonly Attachment[],
  put: (attachment: Attachment) => Promise<string>,
  mark: (id: string, change: Partial<Attachment>) => void,
  problem: (error: unknown) => string,
): Promise<Attachment[] | null> {
  const waiting = attachments.filter((attachment) => !attachment.path);
  for (const attachment of waiting) mark(attachment.id, { upload: 'sending', problem: undefined });
  let sent = [...attachments];
  let failed = false;
  for (const attachment of waiting) {
    try {
      const path = await put(attachment);
      sent = sent.map((known) => (known.id === attachment.id ? { ...known, path } : known));
      mark(attachment.id, { upload: undefined, path });
    } catch (error) {
      failed = true;
      mark(attachment.id, { upload: 'failed', problem: problem(error) });
    }
  }
  return failed ? null : sent;
}
