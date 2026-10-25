import { invokeCommand } from "./host";

/** A host on the account by its key, or undefined for this one. */
export type FilesHost = string | undefined;

export type PathKind = "file" | "dir";

/* The chat's files are on its sharer's computer: none can be read, revealed or checked from here. */
export const fsapi = {
  revealInFinder: (_path: string): Promise<void> =>
    invokeCommand("reveal_in_finder"),
  previewFile: (
    _path: string,
  ): Promise<{ mime: string; size: number; modified: number }> =>
    invokeCommand("preview_file"),
};

export function filesOn(_host: FilesHost) {
  return {
    pathKinds: (_paths: readonly string[]): Promise<(PathKind | null)[]> =>
      invokeCommand("files.kinds"),
  };
}

export function convertFileSrc(_path: string, _protocol?: string): string {
  return "";
}

export function previewUrl(path: string, revision = 0): string {
  const url = convertFileSrc(path, "preview");
  return revision ? `${url}?v=${revision}` : url;
}
