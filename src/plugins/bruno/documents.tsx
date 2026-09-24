import { basename, copyText, files, notify, relativePath, reportError } from "../../plugin-api/host";
import type { PluginDocuments } from "../../plugin-api";
import { FILE_MANAGER_NAME } from "../../plugin-api/ui";
import { findRequest } from "./lib/resolve";
import { useBrunoRuntime } from "./runtime";
import { brunoCloseTab, brunoReorderTab, brunoSelectRequest, brunoSettings, useBruno, viewOf } from "./state";
import "./method.css";

const copy = (text: string, label: string) => void copyText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

/** Each open request is a tab in the workspace strip, named and badged from the collection. */
export const brunoDocuments: PluginDocuments = {
    list(paneId) {
        const view = viewOf(paneId);
        return { ids: view.openPaths, activeId: view.activeRequestPath };
    },

    describe(paneId, path) {
        const { collection } = useBruno.getState();
        const located = collection ? findRequest(collection.tree, path) : null;
        const method = located?.request.method ?? "get";
        return {
            label: located?.request.meta.name || basename(path).replace(/\.bru$/, ""),
            title: path,
            dirty: useBrunoRuntime.getState().drafts[paneId]?.[path] != null,
            icon: <span className={`bruno-method m-${method}`}>{method.toUpperCase()}</span>,
        };
    },

    select: brunoSelectRequest,
    close: brunoCloseTab,
    reorder: brunoReorderTab,

    menu(paneId, path) {
        const open = viewOf(paneId).openPaths;
        const index = open.indexOf(path);
        const close = (paths: string[]) => paths.forEach((each) => brunoCloseTab(paneId, each));
        const others = open.filter((each) => each !== path);
        const toLeft = index > 0 ? open.slice(0, index) : [];
        const toRight = index >= 0 ? open.slice(index + 1) : [];
        const { collectionPath } = brunoSettings.get();
        return [
            { label: "Close", hint: "⌥W", run: () => close([path]) },
            { label: "Close Others", disabled: others.length === 0, run: () => close(others) },
            { label: "Close to the Left", disabled: toLeft.length === 0, run: () => close(toLeft) },
            { label: "Close to the Right", disabled: toRight.length === 0, run: () => close(toRight) },
            { label: "Close All", run: () => close(open) },
            { sep: true },
            { label: "Copy Path", run: () => copy(path, "path") },
            { label: "Copy Relative Path", run: () => copy(relativePath(path, collectionPath) ?? basename(path), "relative path") },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => void files.revealInFinder(path).catch(reportError("reveal")) },
        ];
    },

    subscribe(listener) {
        const stopViews = useBruno.subscribe(listener);
        const stopDrafts = useBrunoRuntime.subscribe(listener);
        return () => {
            stopViews();
            stopDrafts();
        };
    },
};
