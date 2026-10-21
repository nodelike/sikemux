import { id, name } from "../../../src-tauri/plugins/slack/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const SLACK_PLUGIN = { id, name };
export const SLACK_MESSAGES = surfaceKind(SLACK_PLUGIN, "messages");
