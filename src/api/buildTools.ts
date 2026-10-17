import { invokeCommand as invoke } from "./invoke";

export const buildTools = {
    installed: (program: string) => invoke<boolean>("build_tool_installed", { program }),
    install: (program: string) => invoke<string>("install_build_tool", { program }),
};
