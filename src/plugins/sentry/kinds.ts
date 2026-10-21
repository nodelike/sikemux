import { id, name } from "../../../src-tauri/plugins/sentry/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const SENTRY_PLUGIN = { id, name };
export const SENTRY_ISSUES = surfaceKind(SENTRY_PLUGIN, "issues");
