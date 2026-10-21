import { id, name } from "../../../src-tauri/plugins/jira/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const JIRA_PLUGIN = { id, name };
export const JIRA_ISSUES = surfaceKind(JIRA_PLUGIN, "issues");
