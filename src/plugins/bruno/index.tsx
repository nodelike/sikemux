import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { basename } from "../../plugin-api/host";
import { IconBruno, IS_MACOS } from "../../plugin-api/ui";
import { BrunoOverlay } from "./components/BrunoOverlay";
import { brunoDocuments } from "./documents";
import { BRUNO_CLIENT, BRUNO_PLUGIN_ID } from "./kinds";
import {
    activeBrunoPane,
    brunoSaveRequest,
    brunoSettings,
    openBrunoSession,
    openPalette,
    removeBrunoWorkspace,
    requestRun,
    togglePalette,
    viewOf,
} from "./state";

const BrunoPane = lazy(() => import("./components/BrunoPane").then((module) => ({ default: module.BrunoPane })));

/** Runs `action` on the Bruno pane in front; any other pane leaves the key alone. */
const inBruno = (action: (paneId: string) => void) => () => {
    const paneId = activeBrunoPane();
    if (!paneId) return false;
    action(paneId);
    return true;
};

registerFrontendPlugin({
    id: BRUNO_PLUGIN_ID,
    surfaces: [
        {
            kind: BRUNO_CLIENT,
            title: "Bruno",
            icon: (size) => <IconBruno size={size} />,
            render: ({ paneId, visible }) => <BrunoPane paneId={paneId} active={visible} />,
            quickOpen: () => togglePalette("requestPalette"),
            documents: brunoDocuments,
        },
    ],
    open: () => openBrunoSession(),
    openTitle: "Open Bruno",
    openShortcut: "Alt+KeyB",
    shortcuts: [
        {
            name: "save",
            label: "Save request",
            detail: "Save the active Bruno request",
            defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+KeyS`,
            run: inBruno((paneId) => {
                const path = viewOf(paneId).activeRequestPath;
                if (path) void brunoSaveRequest(paneId, path);
            }),
        },
        {
            name: "send",
            label: "Send request",
            detail: "Run the active Bruno request",
            defaultBinding: `${IS_MACOS ? "Meta" : "Ctrl"}+Enter`,
            run: inBruno(requestRun),
        },
        {
            name: "environment",
            label: "Choose environment",
            detail: "Open the Bruno environment picker",
            defaultBinding: "Alt+KeyE",
            run: inBruno(() => openPalette("environmentPalette")),
        },
    ],
    picker: {
        heading: "Bruno workspaces",
        entries: () => {
            const { workspaces, collectionPath } = brunoSettings.get();
            return workspaces
                .filter((path) => path !== collectionPath)
                .map((path) => ({
                    id: path,
                    name: basename(path),
                    sub: path,
                    icon: <IconBruno size={14} />,
                    open: () => openBrunoSession(path),
                    forget: () => removeBrunoWorkspace(path),
                }));
        },
    },
    Overlay: BrunoOverlay,
});
