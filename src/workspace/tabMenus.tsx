import { fsapi } from "../api/fs";
import { copyText } from "../lib/clipboard";
import { basename, relativePath } from "@shared/lib/paths";
import { FILE_MANAGER_NAME } from "@shared/lib/platform";
import type { CtxItem } from "../rail/FileTree";
import * as cmd from "../state/commands";
import { promptDialog } from "../state/dialog";
import { collectPanes } from "../state/layout";
import { editorPaneOf, expandTabRefs, terminalLabel } from "../state/selectors";
import { getState } from "../state/store";
import { notify, reportError } from "../state/toast";
import type { Window as WindowT } from "../state/types";
import { IconClose, IconCopy, IconFolder, IconPencil, IconRotate, IconSquarePlus } from "../ui/Icons";

const copyPath = (text: string, label: string) => copyText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

export async function renameTerminalTab(win: WindowT, titles: Readonly<Record<string, string>>): Promise<void> {
    const home = collectPanes(win.root).find((pane) => pane.kind === "terminal");
    const name = await promptDialog({
        title: "Rename tab",
        label: "Name",
        initial: home ? terminalLabel(win, home, titles) : win.customName || win.name,
        confirmLabel: "Rename",
    });
    if (name !== null) cmd.renameWindow(win.id, name);
}

/** The menu a window's tab opens, wherever the tab is shown: the strip or the rail. */
export function windowTabMenu(sessionId: string, win: WindowT, closeShortcut: string | null): CtxItem[] {
    const state = getState();
    const siblings = expandTabRefs(state.windowsBySession[sessionId] ?? [], state.windows, state.editorViews).flatMap((ref) =>
        ref.doc === undefined && state.windows[ref.id] ? [state.windows[ref.id]] : [],
    );
    const others = siblings.filter((t) => t.id !== win.id && !t.fixed && t.role !== "agent");
    const naming: CtxItem[] =
        win.role === "term"
            ? [
                  { label: "Rename…", icon: <IconPencil size={13} />, run: () => void renameTerminalTab(win, getState().terminalTitles) },
                  ...(win.customName
                      ? [{ label: "Use Automatic Name", icon: <IconRotate size={13} />, run: () => cmd.renameWindow(win.id, "") }]
                      : []),
                  { sep: true },
              ]
            : [];
    return [
        ...naming,
        { label: "Duplicate", icon: <IconSquarePlus size={13} />, run: () => cmd.duplicateWindow(win.id) },
        {
            label: "Close",
            icon: <IconClose size={13} />,
            hint: closeShortcut ?? undefined,
            disabled: win.fixed,
            run: () => cmd.closeWindowById(win.id),
        },
        {
            label: "Close Others",
            icon: <IconClose size={13} />,
            disabled: others.length === 0,
            run: () => cmd.closeWindowsById(others.map((t) => t.id)),
        },
    ];
}

/** The menu an open file's tab opens, wherever the tab is shown. */
export function fileTabMenu(cwd: string, win: WindowT, doc: string, closeShortcut: string | null): CtxItem[] {
    const state = getState();
    const paneId = editorPaneOf(win, state.editorViews);
    const open = state.editorViews[paneId]?.openTabs ?? [];
    const dirty = new Set(state.dirtyEditorPaths[paneId] ?? []);
    const index = open.indexOf(doc);
    const close = (paths: string[]) => paths.forEach((path) => cmd.closeTab({ id: win.id, doc: path }));
    const others = open.filter((path) => path !== doc);
    const toLeft = index > 0 ? open.slice(0, index) : [];
    const toRight = index >= 0 ? open.slice(index + 1) : [];
    const saved = open.filter((path) => !dirty.has(path));
    return [
        { label: "Close", icon: <IconClose size={13} />, hint: closeShortcut ?? undefined, run: () => close([doc]) },
        { label: "Close Others", icon: <IconClose size={13} />, disabled: others.length === 0, run: () => close(others) },
        { label: "Close to the Left", icon: <IconClose size={13} />, disabled: toLeft.length === 0, run: () => close(toLeft) },
        { label: "Close to the Right", icon: <IconClose size={13} />, disabled: toRight.length === 0, run: () => close(toRight) },
        { label: "Close Saved", icon: <IconClose size={13} />, disabled: saved.length === 0, run: () => close(saved) },
        { label: "Close All", icon: <IconClose size={13} />, run: () => close(open) },
        { sep: true },
        { label: "Copy Path", icon: <IconCopy size={13} />, run: () => void copyPath(doc, "path") },
        {
            label: "Copy Relative Path",
            icon: <IconCopy size={13} />,
            run: () => void copyPath(relativePath(doc, cwd) ?? basename(doc), "relative path"),
        },
        { sep: true },
        {
            label: `Reveal in ${FILE_MANAGER_NAME}`,
            icon: <IconFolder size={13} />,
            run: () => void fsapi.revealInFinder(doc).catch(reportError("reveal")),
        },
    ];
}
