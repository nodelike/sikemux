import type { ReactNode } from "react";
import {
    IconAgent,
    IconBoard,
    IconCommit,
    IconDownload,
    IconEditor,
    IconEye,
    IconGit,
    IconSearch,
    IconSettings,
    IconTerminal,
    IconWindow,
} from "@shared/ui/Icons";
import type { CoreWindowRole, WindowRole } from "../state/windowRoles";
import { isPluginKind } from "../plugins/kinds";
import { pluginSurface } from "../plugins/registry";

const WINDOW_ICONS: Record<CoreWindowRole, (props: { size?: number }) => ReactNode> = {
    term: IconTerminal,
    files: IconEditor,
    git: IconGit,
    diff: IconCommit,
    search: IconSearch,
    "ssh-config": IconWindow,
    named: IconWindow,
    agent: IconAgent,
    shared: IconEye,
    board: IconBoard,
    "project-settings": IconSettings,
    install: IconDownload,
};

export function WindowIcon({ role, size }: { role: WindowRole; size?: number }) {
    if (isPluginKind(role)) return pluginSurface(role)?.icon(size ?? 14) ?? <IconWindow size={size} />;
    const Icon = WINDOW_ICONS[role];
    return <Icon size={size} />;
}
