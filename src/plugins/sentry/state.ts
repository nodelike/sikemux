import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import type { IssuePeriod, IssueSort, IssueStatusFilter } from "./api";
import { SENTRY_ISSUES } from "./kinds";

export type SentrySection = "issues" | "releases";

export interface SentryView {
    section: SentrySection;
    /** The project shown, by slug; none for every project, or the one the sign-in keeps. */
    project: string | null;
    environment: string | null;
    status: IssueStatusFilter;
    sort: IssueSort;
    period: IssuePeriod;
    /** Sentry search words. */
    query: string;
    /** The issue open, by its numeric id. */
    issue: string | null;
    sideWidth: number | null;
}

const views = definePaneViews<SentryView>({
    section: "issues",
    project: null,
    environment: null,
    status: "unresolved",
    sort: "date",
    period: "24h",
    query: "",
    issue: null,
    sideWidth: null,
});

export const useSentryView = views.use;
export const updateSentryView = views.update;

export const openSentry = (): string | null => openSurface(SENTRY_ISSUES);
