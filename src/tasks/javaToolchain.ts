import type { JavaRuntime, JavaRuntimes } from "../api/toolchains";

export interface JavaChoice {
    readonly required: number;
    /** The Java the task will run on, or null when nothing installed is new enough. */
    readonly runtime: JavaRuntime | null;
    /** The Java a plain shell would have used. */
    readonly current: JavaRuntime | null;
}

/** The exact version asked for, else the shell's own Java if it is new enough, else the closest newer one. */
export function chooseJava(required: number, runtimes: JavaRuntimes): JavaChoice {
    const current = runtimes.default;
    const exact = current?.major === required ? current : runtimes.installed.find((runtime) => runtime.major === required);
    const newer = runtimes.installed.filter((runtime) => runtime.major > required).sort((left, right) => left.major - right.major)[0];
    const runtime = exact ?? (current && current.major > required ? current : newer) ?? null;
    return { required, runtime, current };
}
