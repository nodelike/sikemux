import type { Issue, IssueDetail, Project, SentryStatus } from "./api";

export const signedIn: SentryStatus = {
    configured: true,
    ok: true,
    authFailed: false,
    message: null,
    url: "https://sentry.io",
    org: "acme",
    orgName: "Acme",
    project: null,
    user: "Ankit",
};

export const signedOut: SentryStatus = { ...signedIn, configured: false, ok: false, org: "", orgName: "", user: "" };

export const WEB: Project = { id: "11", slug: "web", name: "Web", platform: "javascript-nextjs" };
export const API: Project = { id: "12", slug: "api", name: "API", platform: "python" };

export function issue(id: string, patch: Partial<Issue> = {}): Issue {
    return {
        id,
        shortId: `WEB-${id}`,
        title: `Error ${id}`,
        culprit: "app/page.tsx in Page",
        kind: null,
        message: null,
        level: "error",
        status: "unresolved",
        substatus: null,
        events: 10,
        users: 3,
        firstSeen: "2026-10-12T08:00:00Z",
        lastSeen: "2026-10-19T08:00:00Z",
        project: "web",
        link: `https://acme.sentry.io/issues/${id}/`,
        assignee: null,
        unhandled: false,
        trend: [1, 2, 3],
        ...patch,
    };
}

export const TYPE_ERROR = issue("3F", {
    title: "TypeError: Cannot read properties of undefined (reading 'total')",
    kind: "TypeError",
    message: "Cannot read properties of undefined (reading 'total')",
    substatus: "regressed",
    events: 1284,
    users: 312,
    unhandled: true,
});

export const TYPE_ERROR_DETAIL: IssueDetail = {
    ...TYPE_ERROR,
    firstRelease: "web@4.12.0",
    lastRelease: "web@4.13.0",
    comments: 0,
    event: {
        id: "e1",
        at: "2026-10-19T08:00:00Z",
        message: null,
        release: "web@4.13.0",
        environment: "production",
        request: "GET https://shop.acme.dev/checkout",
        contexts: [["Browser", "Chrome 128"]],
        tags: [
            ["environment", "production"],
            ["browser", "Chrome 128"],
        ],
        breadcrumbs: [{ at: "2026-10-19T07:59:59Z", category: "fetch", level: "warning", message: "GET /api/orders/draft [404]" }],
        exceptions: [
            {
                kind: "TypeError",
                value: "Cannot read properties of undefined (reading 'total')",
                handled: false,
                frames: [
                    {
                        file: "node_modules/react-dom/index.js",
                        module: null,
                        function: "commitRoot",
                        line: 4012,
                        column: null,
                        inApp: false,
                        context: [],
                    },
                    {
                        file: "node_modules/react-dom/index.js",
                        module: null,
                        function: "renderWithHooks",
                        line: 5014,
                        column: null,
                        inApp: false,
                        context: [],
                    },
                    {
                        file: "app/checkout/summary.ts",
                        module: null,
                        function: "totalOf",
                        line: 12,
                        column: 22,
                        inApp: true,
                        context: [
                            [11, "  const lines = order?.lines;"],
                            [12, "  return lines.total;"],
                        ],
                    },
                ],
            },
        ],
    },
};
