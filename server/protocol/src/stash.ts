/** The most the server keeps of one stashed file. */
export const MAX_STASH_FILE_BYTES = 25 * 1024 * 1024;
/** The most the server keeps of one account's stashed files together. */
export const MAX_STASH_ACCOUNT_BYTES = 1024 * 1024 * 1024;
/** The longest text the server takes as text or a prompt, as StashAddText and StashAddPrompt say; longer text goes as a file. */
export const MAX_STASH_TEXT = 16_000;

/** The headers an upload says what it is with. */
export const STASH_HEADERS = {
  name: "x-stash-name",
  note: "x-stash-note",
  target: "x-stash-target",
  prompt: "x-stash-prompt",
} as const;

/** An upload's headers: the free text URI-encoded, since a header takes only ASCII, and a stashed item's id as it is. */
export function stashUploadHeaders(file: {
  name: string;
  note?: string;
  target?: unknown;
  prompt?: string;
}): Record<string, string> {
  return {
    [STASH_HEADERS.name]: encodeURIComponent(file.name),
    ...(file.note
      ? { [STASH_HEADERS.note]: encodeURIComponent(file.note) }
      : {}),
    ...(file.target
      ? {
          [STASH_HEADERS.target]: encodeURIComponent(
            JSON.stringify(file.target),
          ),
        }
      : {}),
    ...(file.prompt ? { [STASH_HEADERS.prompt]: file.prompt } : {}),
  };
}
