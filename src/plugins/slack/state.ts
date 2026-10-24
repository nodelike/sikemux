import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { SLACK_MESSAGES } from "./kinds";

export interface SlackView {
    /** The workspace shown; empty for the default one. */
    workspace: string;
    channel: string | null;
    /** The thread open beside the channel. */
    thread: { channel: string; ts: string } | null;
}

const views = definePaneViews<SlackView>({ workspace: "", channel: null, thread: null });

export const useSlackView = views.use;
export const updateSlackView = views.update;

export const openSlack = (): string | null => openSurface(SLACK_MESSAGES);
