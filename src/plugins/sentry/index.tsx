import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { SentryMark } from "./components/SentryMark";
import { SENTRY_ISSUES, SENTRY_PLUGIN } from "./kinds";
import { sentry } from "./resources";
import { openSentry } from "./state";

const SentryPane = lazy(() => import("./components/SentryPane").then((module) => ({ default: module.SentryPane })));

registerFrontendPlugin({
    id: SENTRY_PLUGIN.id,
    surfaces: [
        {
            kind: SENTRY_ISSUES,
            title: SENTRY_PLUGIN.name,
            icon: (size) => <SentryMark size={size} className="icon-sentry" />,
            render: ({ paneId, visible }) => <SentryPane paneId={paneId} active={visible} />,
        },
    ],
    open: openSentry,
    openTitle: `Open ${SENTRY_PLUGIN.name}`,
    mark: (size) => <SentryMark size={size} className="icon-sentry" />,
    linkHosts: ["sentry.io"],
    useSignIn: sentry.useSignIn,
});
