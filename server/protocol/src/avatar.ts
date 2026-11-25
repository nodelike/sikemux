/** The hash Boring Avatars draws a picture from. */
export function hashOf(id: string): number {
  let hash = 0;
  for (let at = 0; at < id.length; at++)
    hash = ((hash << 5) - hash + id.charCodeAt(at)) | 0;
  return Math.abs(hash);
}

/** What a person's picture is drawn from: the same for everyone who sees them, and not their account's id. */
export const avatarSeed = (userId: string): string =>
  hashOf(userId).toString(36);
