import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import type { PluginFailure } from "../api/plugins";
import { invalidate, resource, type ResourceDef } from "../state/resources";
import { isSignedOutFailure } from "./backend";
import { resourceKind, type PluginIdentity } from "./identity";

type Answer<T> = { value: T } | { failed: PluginFailure };

/** Each view's resource by the path a `pluginView` change names it with. */
const kinds = new Map<string, string>();
let following = false;

/** The core says when a view changed, so it is read again then and at no other time. */
function follow(): void {
    if (following) return;
    following = true;
    onCoreChanged("pluginView", ({ paths }) => {
        const moved = new Set((paths ?? [...kinds.keys()]).map((path) => kinds.get(path)));
        invalidate((kind) => moved.has(kind));
    });
}

/**
 * A view the plugin's manifest names, which the core reads, keeps and reads again while `slot` (a pane) shows it.
 * A failure that says the person signed out has the plugin's sign-in read again.
 */
export function pluginView<Args extends unknown[], T>(
    plugin: PluginIdentity,
    method: string,
    params: (...args: Args) => unknown,
): ResourceDef<[slot: string, ...Args], T> {
    const kind = resourceKind(plugin.id, method);
    kinds.set(`${plugin.id}/${method}`, kind);
    return resource({
        kind,
        fetch: async (slot: string, ...args: Args): Promise<T> => {
            follow();
            const answer = await coreCall<Answer<T>>("pluginView", { plugin: plugin.id, method, params: params(...args), slot });
            if ("value" in answer) return answer.value;
            if (isSignedOutFailure(answer.failed)) invalidate((each) => each === resourceKind(plugin.id, "status"));
            throw answer.failed;
        },
    });
}

/** Has the core read the plugin's views again now, each of `method`'s or all; those that changed come back as a change. */
export function readViewsAgain(plugin: PluginIdentity, method?: string): Promise<void> {
    return coreCall<void>("pluginViewsRead", { plugin: plugin.id, method });
}
