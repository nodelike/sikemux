import { BLANK_URL } from "../api/browser";
import { basename, relativePath } from "../lib/paths";
import { FILE_MANAGER_NAME } from "../lib/platform";
import type { CtxItem } from "../rail/FileTree";
import type { DeskItem } from "../state/desks";

export interface DeskTabActions {
    copy: (text: string, label: string) => void;
    reveal: (path: string) => void;
    close: () => void;
}

/** What right-clicking a desk tab offers: a file's path, absolute or within the project, or a page's link. */
export function deskTabMenu(item: DeskItem, projectRoot: string | null, actions: DeskTabActions): CtxItem[] {
    const close: CtxItem = { label: "Close", run: actions.close };
    if (item.kind === "file") {
        const relative = (projectRoot && relativePath(item.path, projectRoot)) || basename(item.path);
        return [
            { label: "Copy Path", run: () => actions.copy(item.path, "path") },
            { label: "Copy Relative Path", run: () => actions.copy(relative, "relative path") },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => actions.reveal(item.path) },
            { sep: true },
            close,
        ];
    }
    if (item.kind === "browser") {
        const url = item.tab.url;
        return [{ label: "Copy Link", disabled: !url || url === BLANK_URL, run: () => actions.copy(url, "link") }, { sep: true }, close];
    }
    return [close];
}
