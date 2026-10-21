import { id, name } from "../../../src-tauri/plugins/kubernetes/manifest.json";
import { surfaceKind } from "../../plugin-api/identity";

export const KUBERNETES_PLUGIN = { id, name };
export const KUBERNETES_CLUSTER = surfaceKind(KUBERNETES_PLUGIN, "cluster");
