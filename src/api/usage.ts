import { invokeCommand as invoke } from "./invoke";
import { getState } from "../state/store";
import { swallow } from "../state/toast";

export function reportActive(): Promise<void> {
    const { shareUsageData, updateChannel } = getState();
    if (!shareUsageData) return Promise.resolve();
    return invoke<void>("usage_report_active", { channel: updateChannel }).catch(swallow("usage report"));
}
