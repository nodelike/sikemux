/** An in-memory disk, emptied with `clear` between tests. It outlives `vi.resetModules`, as a disk outlives a relaunch. */
const shared = globalThis as { sikemuxTestFiles?: Map<string, string> };
const files = (shared.sikemuxTestFiles ??= new Map<string, string>());

export const Paths = { document: { uri: 'file:///document/' }, cache: { uri: 'file:///cache/' } };

function join(parts: unknown[]): string {
  return parts
    .map((part) => (typeof part === 'string' ? part : (part as { uri: string }).uri))
    .reduce((path, part) => (path ? `${path.replace(/\/$/, '')}/${part.replace(/^file:\/\/\//, '')}` : part), '');
}

export class File {
  readonly uri: string;

  constructor(...parts: unknown[]) {
    this.uri = join(parts);
  }

  get exists() {
    return files.has(this.uri);
  }

  async text() {
    const text = files.get(this.uri);
    if (text === undefined) throw new Error(`no file at ${this.uri}`);
    return text;
  }

  write(content: string) {
    files.set(this.uri, content);
  }

  delete() {
    files.delete(this.uri);
  }
}

export class Directory {
  readonly uri: string;

  constructor(...parts: unknown[]) {
    this.uri = `${join(parts).replace(/\/$/, '')}/`;
  }

  create() {}
}

export const clear = () => files.clear();
