import { invokeCommand as invoke } from "./invoke";

/** A page an agent showed, as `page_show` hands it back. */
export interface PageRef {
    id: string;
    title: string;
    /** The page's height at the reply width, when the agent measured it. */
    height?: number;
}

export const pagesApi = {
    theme: (dark: boolean, variables: Record<string, string>) => invoke<void>("page_theme", { dark, variables }),
    /** Keeps the page for the agent's reply, closing the tab its draft was checked in. */
    publish: (agentId: string | null, path: string, title: string, height?: number) =>
        invoke<PageRef>("page_publish", { agentId, path, title, height: height ?? null }),
    /** Opens the page in a tab on the agent's desk and answers with the tab. */
    open: (agentId: string, id: string) => invoke<string>("page_open", { agentId, id }),
};
