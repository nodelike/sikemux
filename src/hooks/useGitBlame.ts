import { useEffect } from "react";
import type { EditorView } from "@codemirror/view";
import { refreshBlame, setBlameContext } from "../editor/gitBlame";
import { isPreviewPath } from "../editor/viewers/fileKinds";
import { subscribe } from "../state/bus";
import { relativePath } from "../lib/paths";

/**
 * Bind the editor's inline git-blame to the active file: points the blame
 * extension at it (which triggers an initial fetch) and re-blames on
 * `git-refresh` (commit / checkout / external change). Doc-edit re-blame is
 * handled inside the extension itself.
 */
export function useGitBlame(viewGetter: () => EditorView | null, cwd: string, activePath: string | null) {
    useEffect(() => {
        const view = viewGetter();
        if (!view) return;
        const rel = activePath && cwd ? relativePath(activePath, cwd) : null;
        if (!activePath || !cwd || isPreviewPath(activePath) || !rel) {
            setBlameContext(view, null);
            return;
        }
        setBlameContext(view, { repo: cwd, path: rel });
        return subscribe("git-refresh", (e) => {
            if (e.repo !== cwd) return;
            refreshBlame(viewGetter());
        });
        // eslint-disable-next-line react-hooks/exhaustive-deps -- callers pass a new getter every render for the same editor
    }, [activePath, cwd]);
}
