import { createContext, useContext } from "react";
import { joinPath } from "../lib/paths";
import { requestOpenFile } from "../state/commands";

const LocalRepoContext = createContext<string | null>(null);

/** The project folder, given only while the repository on screen is that folder's own. */
export const LocalRepoProvider = LocalRepoContext.Provider;

/** Opens a path from the host in the editor, at a line when there is one; null when the files are not checked out here. */
export function useOpenLocalFile(): ((path: string, line?: number | null) => void) | null {
    const cwd = useContext(LocalRepoContext);
    if (!cwd) return null;
    return (path, line) => requestOpenFile(joinPath(cwd, path), line ?? undefined);
}
