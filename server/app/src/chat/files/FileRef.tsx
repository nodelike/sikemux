import { createContext, useContext, useMemo, type ReactNode } from "react";
import { FileIcon, FileTypeIcon } from "./FileIcon";
import { IconFolder } from "@shared/ui/Icons";
import { basename } from "@shared/lib/paths";
import {
  parsePathRef,
  type PathRef,
  type PathRoots,
} from "@shared/chat/filePath";

export type PathKind = "file" | "dir";

const PathRootsContext = createContext<PathRoots>({ cwd: "" });

/** Where a transcript's relative paths are relative to. */
export function PathRootsProvider({
  cwd,
  home,
  children,
}: PathRoots & { children: ReactNode }) {
  const roots = useMemo(() => ({ cwd, home }), [cwd, home]);
  return (
    <PathRootsContext.Provider value={roots}>
      {children}
    </PathRootsContext.Provider>
  );
}

export function usePathRoots(): PathRoots {
  return useContext(PathRootsContext);
}

/**
 * Reads a string as a file reference. A shared chat's files are on its sharer's computer, where
 * nothing can be looked up, so whatever names a path is taken to be one.
 */
export function useFileRef(
  raw: string | null | undefined,
): { ref: PathRef; state: PathKind } | null {
  const roots = usePathRoots();
  return useMemo(() => {
    const ref = raw ? parsePathRef(raw, roots) : null;
    return ref ? { ref, state: ref.path.endsWith("/") ? "dir" : "file" } : null;
  }, [raw, roots]);
}

/** A file the transcript names: its icon, what the agent called it, and the line when one was given. */
export function ChatFileRef({
  refers,
  state,
  label,
  size = 18,
  className = "chat-file-ref",
  tile = false,
}: {
  refers: PathRef;
  state: PathKind;
  label: ReactNode;
  size?: number;
  className?: string;
  tile?: boolean;
}) {
  return (
    <span
      className={className}
      data-kind={state}
      role={tile ? "img" : undefined}
      aria-label={tile ? basename(refers.path) : undefined}
      title={
        refers.line === undefined
          ? refers.path
          : `${refers.path}:${refers.line}`
      }
    >
      {state === "dir" ? (
        <span className="file-glyph" aria-hidden="true">
          <IconFolder size={size} />
        </span>
      ) : tile ? (
        <FileTypeIcon name={basename(refers.path)} size={size} />
      ) : (
        <FileIcon name={basename(refers.path)} size={Math.round(size * 0.8)} />
      )}
      {!tile && <span className="chat-file-ref-name">{label}</span>}
    </span>
  );
}
