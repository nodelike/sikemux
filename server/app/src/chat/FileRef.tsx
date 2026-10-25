import {
  createContext,
  useContext,
  useMemo,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";
import { fsapi, type FilesHost } from "./page/files";
import { FileIcon, FileTypeIcon } from "./ui/FileIcon";
import { TreeContextMenu, type CtxItem } from "./page/fileTree";
import {
  IconCopy,
  IconEditor,
  IconFile,
  IconFolder,
  IconPanelRight,
} from "@shared/ui/Icons";
import { copyText } from "./lib/clipboard";
import { basename, dirname, relativePath } from "@shared/lib/paths";
import { FILE_MANAGER_NAME } from "@shared/lib/platform";
import * as cmd from "./page/commands";
import type { FilesPlace } from "./page/commands";
import { notify, reportError } from "./page/toast";
import {
  parsePathRef,
  type PathRef,
  type PathRoots,
} from "@shared/chat/filePath";
import { useRemoteOrigin } from "./origin";
import { usePathState, type PathState } from "./pathExistence";

const PathRootsContext = createContext<PathRoots>({ cwd: "" });
const DeskOwnerContext = createContext<string | null>(null);
const ChatHostContext = createContext<FilesHost>(undefined);

/** Where a transcript's relative paths are relative to, the other host that holds them, if any, and whose desk its files open on. */
export function PathRootsProvider({
  cwd,
  home,
  agentId,
  host,
  children,
}: PathRoots & { agentId?: string; host?: FilesHost; children: ReactNode }) {
  const roots = useMemo(() => ({ cwd, home }), [cwd, home]);
  return (
    <PathRootsContext.Provider value={roots}>
      <ChatHostContext.Provider value={host}>
        <DeskOwnerContext.Provider
          value={host === undefined ? (agentId ?? null) : null}
        >
          {children}
        </DeskOwnerContext.Provider>
      </ChatHostContext.Provider>
    </PathRootsContext.Provider>
  );
}

export function usePathRoots(): PathRoots {
  return useContext(PathRootsContext);
}

/**
 * Reads a string as a file reference and looks it up. Both halves are null
 * until a path that exists comes back, so text that only reads like a filename
 * stays text.
 */
export function useFileRef(
  raw: string | null | undefined,
): { ref: PathRef; state: PathState } | null {
  const roots = usePathRoots();
  const ref = useMemo(
    () => (raw ? parsePathRef(raw, roots) : null),
    [raw, roots],
  );
  const state = usePathState(ref?.path ?? null, useContext(ChatHostContext));
  if (!ref || state === null || state === "missing") return null;
  return { ref, state };
}

function copy(value: string, label: string) {
  void copyText(value)
    .then(() => notify("success", `copied ${label}`))
    .catch(reportError("copy"));
}

/** Opens a file on the agent's desk when there is one to open it on, and in the workspace editor otherwise, of the copy on `place.host` when it is another host's. */
export function openFileRef(
  ref: PathRef,
  state: PathState,
  deskAgentId: string | null = null,
  place?: FilesPlace,
): void {
  if (state === "dir") {
    if (!place)
      void fsapi.revealInFinder(ref.path).catch(reportError("reveal"));
    return;
  }
  const line = ref.line === undefined ? undefined : ref.line - 1;
  const column = ref.column === undefined ? undefined : ref.column - 1;
  if (deskAgentId) cmd.openFileOnDesk(deskAgentId, ref.path, line, column);
  else cmd.requestOpenFile(ref.path, line, column, place);
}

function menuItems(
  ref: PathRef,
  state: PathState,
  cwd: string,
  deskAgentId: string | null,
  remote: boolean,
  place?: FilesPlace,
): CtxItem[] {
  const relative = relativePath(ref.path, cwd) ?? basename(ref.path);
  const reveal: CtxItem = {
    label: `Reveal in ${FILE_MANAGER_NAME}`,
    icon: <IconFolder size={13} />,
    run: () => void fsapi.revealInFinder(ref.path).catch(reportError("reveal")),
  };
  const copies: CtxItem[] = [
    {
      label: "Copy Path",
      icon: <IconCopy size={13} />,
      run: () => copy(ref.path, "path"),
    },
    {
      label: "Copy Relative Path",
      icon: <IconCopy size={13} />,
      run: () => copy(relative, "relative path"),
    },
    {
      label: "Copy Name",
      icon: <IconCopy size={13} />,
      run: () => copy(basename(ref.path), "name"),
    },
  ];
  if (remote || (place && state === "dir")) return copies;
  if (place)
    return [
      {
        label: "Open",
        icon: <IconFile size={13} />,
        run: () => openFileRef(ref, state, null, place),
      },
      { sep: true },
      ...copies,
    ];
  if (state === "dir") return [reveal, { sep: true }, ...copies];
  return [
    ...(deskAgentId
      ? [
          {
            label: "Open on Desk",
            icon: <IconPanelRight size={13} />,
            run: () => openFileRef(ref, state, deskAgentId),
          },
        ]
      : []),
    {
      label: deskAgentId ? "Open in Editor" : "Open",
      icon: deskAgentId ? <IconEditor size={13} /> : <IconFile size={13} />,
      run: () => openFileRef(ref, state),
    },
    { sep: true },
    reveal,
    {
      label: "Open Containing Folder",
      icon: <IconFolder size={13} />,
      run: () =>
        void fsapi
          .revealInFinder(dirname(ref.path))
          .catch(reportError("reveal")),
    },
    { sep: true },
    ...copies,
  ];
}

/**
 * A file the transcript names: its icon, what the agent called it, and the
 * line when one was given. A click puts it on the agent's desk and a double
 * click opens it in the workspace editor; its menu is the one the file tree
 * offers.
 */
export function ChatFileRef({
  refers,
  state,
  label,
  size = 18,
  className = "chat-file-ref",
  tile = false,
}: {
  refers: PathRef;
  state: PathState;
  label: ReactNode;
  size?: number;
  className?: string;
  tile?: boolean;
}) {
  const { cwd } = usePathRoots();
  const deskAgentId = useContext(DeskOwnerContext);
  const host = useContext(ChatHostContext);
  const place = host === undefined ? undefined : { host, cwd };
  const remote = useRemoteOrigin() !== null;
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const openMenu = (event: MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    setMenu({ x: event.clientX, y: event.clientY });
  };
  return (
    <>
      <button
        type="button"
        className={className}
        data-kind={state}
        aria-label={tile ? basename(refers.path) : undefined}
        title={
          refers.line === undefined
            ? refers.path
            : `${refers.path}:${refers.line}`
        }
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          if (event.detail < 2 && !remote)
            openFileRef(refers, state, deskAgentId, place);
        }}
        onDoubleClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          if (state !== "dir" && !remote)
            openFileRef(refers, state, null, place);
        }}
        onContextMenu={openMenu}
      >
        {state === "dir" ? (
          <span className="file-glyph" aria-hidden="true">
            <IconFolder size={size} />
          </span>
        ) : tile ? (
          <FileTypeIcon name={basename(refers.path)} size={size} />
        ) : (
          <FileIcon
            name={basename(refers.path)}
            size={Math.round(size * 0.8)}
          />
        )}
        {!tile && <span className="chat-file-ref-name">{label}</span>}
      </button>
      {menu && (
        <TreeContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(refers, state, cwd, deskAgentId, remote, place)}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
