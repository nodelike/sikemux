import { useEffect } from "react";
import { browserApi } from "../api/browser";
import * as cmd from "./commands";

export function useBrowserReveal(): void {
    useEffect(() => {
        const controller = new AbortController();
        void browserApi.subscribeActing((agentId) => cmd.revealBrowserPane(agentId), controller.signal).catch(() => {});
        return () => controller.abort();
    }, []);
}
