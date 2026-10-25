import type { DeviceAccess } from "../wire.generated";

export const ACCESS_OPTIONS: readonly { value: DeviceAccess; label: string; detail: string }[] = [
    { value: "full", label: "Full control", detail: "Start chats, answer agents, type in terminals" },
    { value: "watch", label: "Watch only", detail: "See chats and terminals, answer what agents ask" },
];

const PLATFORM_NAMES: Record<string, string> = { ios: "iOS", android: "Android", macos: "macOS", linux: "Linux", web: "Web" };

export function platformName(platform: string): string {
    return PLATFORM_NAMES[platform] ?? platform;
}
