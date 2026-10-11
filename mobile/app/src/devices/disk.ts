import { File } from 'expo-file-system';

/** Replaces `file` whole: a crash part way leaves the old copy or the new one, never half of each. */
export function writeWhole(file: File, text: string) {
  const next = new File(`${file.uri}.next`);
  next.write(text);
  next.move(file, { overwrite: true });
}

export type Read<T> = { value: T | undefined; damaged: boolean };

/** What `file` holds, or nothing when it is missing. One that cannot be read is moved aside and reported as damaged. */
export async function readWhole<T>(file: File, valid: (value: unknown) => value is T): Promise<Read<T>> {
  const next = new File(`${file.uri}.next`);
  // A write that stopped between removing the old copy and moving the new one in leaves only the new one.
  const source = file.exists ? new File(file.uri) : next.exists ? next : undefined;
  if (!source) return { value: undefined, damaged: false };
  try {
    const value: unknown = JSON.parse(await source.text());
    if (valid(value)) return { value, damaged: false };
  } catch {}
  try {
    source.move(new File(`${file.uri}.damaged`), { overwrite: true });
  } catch {
    source.delete();
  }
  return { value: undefined, damaged: true };
}
