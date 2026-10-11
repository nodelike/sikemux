import { lazy } from "react";
import { registerFrontendPlugin } from "../../plugin-api";
import { DatabaseMark } from "./components/DatabaseMark";
import { DATABASE_BROWSER, DATABASE_PLUGIN_ID } from "./kinds";
import { openDatabase } from "./state";

const DatabasePane = lazy(() => import("./components/DatabasePane").then((module) => ({ default: module.DatabasePane })));

registerFrontendPlugin({
    id: DATABASE_PLUGIN_ID,
    surfaces: [
        {
            kind: DATABASE_BROWSER,
            title: "Database",
            icon: (size) => <DatabaseMark size={size} />,
            render: ({ paneId, visible }) => <DatabasePane paneId={paneId} active={visible} />,
        },
    ],
    open: openDatabase,
    openTitle: "Open Database",
});
