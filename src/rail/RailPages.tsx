import type { ReactNode } from "react";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import type { TabRailPage } from "../state/types";
import { IconAgents, IconFile, IconTerminal } from "@shared/ui/Icons";
import { SwipePages } from "../ui/SwipePages";

/** The tab rail's pages: the project's agents, its terminals and its files. */
export function RailPages({ pages }: { pages: Record<TabRailPage, ReactNode> }) {
    const page = useStore((s) => s.tabRailPage);
    return (
        <SwipePages
            label="Rail pages"
            current={page}
            onChange={(id) => cmd.setTabRailPage(id as TabRailPage)}
            pages={[
                { id: "agents", label: "Agents", icon: <IconAgents size={15} />, content: pages.agents },
                { id: "terminals", label: "Terminals", icon: <IconTerminal size={15} />, content: pages.terminals },
                { id: "files", label: "Files", icon: <IconFile size={15} />, content: pages.files },
            ]}
        />
    );
}
