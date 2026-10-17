import { invokeCommand as invoke } from "./invoke";

export interface JavaRuntime {
    home: string;
    major: number;
}

export interface JavaRuntimes {
    default: JavaRuntime | null;
    installed: JavaRuntime[];
}

export const toolchains = {
    javaRuntimes: () => invoke<JavaRuntimes>("java_runtimes"),
    installJava: (major: number) => invoke<JavaRuntime>("install_java", { major }),
};
