import { useEffect } from "react";
import { browserApi } from "../api/browser";
import * as cmd from "./commands";
import { swallow } from "./toast";

export function useBrowserReveal(): void {
    useEffect(() => {
        const controller = new AbortController();
        void browserApi.subscribeActing((agentId) => cmd.showDeskBrowser(agentId), controller.signal).catch(swallow("follow browser acting"));
        return () => controller.abort();
    }, []);
}
