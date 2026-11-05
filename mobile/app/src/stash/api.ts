import { isDrawnType } from '@shared/lib/imagePaths';
import { Directory, File, Paths, UploadTask, UploadType } from 'expo-file-system';
import type { StashItem, StashTarget } from '@sikemux/protocol';
import { answerProblem, ApiProblem, bearer } from '@shared/account/api';

import { api, OFFLINE, type TokenSource } from '@/account/api';
import { apiUrl } from '@/account/config';
import type { Attachment } from '@/chat/uploads';

export type { StashItem, StashTarget };

/** Stashes a file from this phone as its bytes, reporting how much has gone. */
export async function stashFile(
  token: TokenSource,
  file: { uri: string; name: string; mime: string },
  extra: { note?: string; target?: StashTarget; prompt?: string },
  options: { onProgress?: (share: number) => void; signal?: AbortSignal } = {},
): Promise<StashItem> {
  const headers: Record<string, string> = {
    authorization: `Bearer ${await bearer(token)}`,
    'content-type': file.mime,
    'x-stash-name': encodeURIComponent(file.name),
  };
  if (extra.note) headers['x-stash-note'] = encodeURIComponent(extra.note);
  if (extra.target) headers['x-stash-target'] = encodeURIComponent(JSON.stringify(extra.target));
  if (extra.prompt) headers['x-stash-prompt'] = extra.prompt;
  const task = new UploadTask(new File(file.uri), `${apiUrl()}/v1/stash`, {
    httpMethod: 'POST',
    uploadType: UploadType.BINARY_CONTENT,
    headers,
    signal: options.signal,
    onProgress: ({ bytesSent, totalBytes }) => {
      if (totalBytes > 0) options.onProgress?.(bytesSent / totalBytes);
    },
  });
  let answer;
  try {
    answer = await task.uploadAsync();
  } catch (error) {
    if (options.signal?.aborted) throw error;
    throw new ApiProblem(OFFLINE);
  } finally {
    task.release();
  }
  if (answer.status < 200 || answer.status >= 300) throw answerProblem(answer.status, readJson(answer.body));
  return JSON.parse(answer.body) as StashItem;
}

function readJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** Brings a stashed file down to this phone, ready to go with a message like one picked here. */
export async function fetchStashFile(token: TokenSource, item: StashItem): Promise<Attachment> {
  const folder = new Directory(Paths.cache, 'stash', item.id);
  if (!folder.exists) folder.create({ intermediates: true });
  const name = item.name ?? 'file';
  const destination = new File(folder, name);
  if (!destination.exists) {
    try {
      await File.downloadFileAsync(`${apiUrl()}/v1/stash/${encodeURIComponent(item.id)}/file`, destination, {
        headers: { authorization: `Bearer ${await bearer(token)}` },
      });
    } catch (error) {
      if (error instanceof ApiProblem) throw error;
      throw new ApiProblem(/\b[45]\d\d\b/.test(String(error)) ? "Couldn't get this from your stash." : OFFLINE);
    }
  }
  const mime = item.mime ?? 'application/octet-stream';
  return {
    id: `stash-${item.id}`,
    kind: isDrawnType(mime) ? 'image' : 'file',
    name,
    mime,
    uri: destination.uri,
    size: destination.size ?? item.size ?? 0,
    stashId: item.id,
  };
}

/**
 * Stashes a whole prompt as one item: its text, then each file under it. The server shows it only once
 * every file is in; one that could not finish is taken back.
 */
export async function stashPrompt(
  token: TokenSource,
  prompt: { text: string; files: readonly { uri: string; name: string; mime: string }[] },
): Promise<StashItem> {
  const item = await api(token).addStash({ kind: 'prompt', text: prompt.text, attachmentCount: prompt.files.length });
  try {
    for (const file of prompt.files) await stashFile(token, file, { prompt: item.id });
  } catch (error) {
    await api(token)
      .deleteStash(item.id)
      .catch(() => {});
    throw error;
  }
  return item;
}
