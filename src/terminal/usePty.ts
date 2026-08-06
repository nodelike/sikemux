import { useEffect, useRef, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";
import { registerPtyDrop } from "../state/dropRegistry";
import { IS_WINDOWS } from "../lib/platform";
import type { PtyContext } from "../state/types";

function shellPathArgument(path: string): string {
    return IS_WINDOWS ? `'${path.replaceAll("'", "''")}'` : path.replace(/([\s'"\\])/g, "\\$1");
}

export function usePty(opts: {
    cwd?: string;
    startup?: string;
    hostRef: RefObject<HTMLDivElement | null>;
    spawnWhen?: boolean;
    context?: PtyContext;
}): RefObject<Promise<number> | null> {
    const { cwd, startup, hostRef, spawnWhen = true, context } = opts;
    const readyRef = useRef<Promise<number> | null>(null);
    const pidRef = useRef<number | null>(null);
    const spawnedRef = useRef(false);
    const disposedRef = useRef(false);
    const unregisterDropRef = useRef<(() => void) | null>(null);

    useEffect(() => {
        const host = hostRef.current;
        if (host) {
            unregisterDropRef.current = registerPtyDrop(host, (paths) => {
                const pid = pidRef.current;
                if (pid === null || paths.length === 0) return;
                const body = paths.map(shellPathArgument).join(" ");
                void invoke("pty_write", {
                    id: pid,
                    data: `\x1b[200~${body}\x1b[201~`,
                });
            });
        }

        return () => {
            disposedRef.current = true;
            const id = pidRef.current;
            pidRef.current = null;
            unregisterDropRef.current?.();
            unregisterDropRef.current = null;
            if (id !== null) void invoke("pty_kill", { id });
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    useEffect(() => {
        if (!spawnWhen || spawnedRef.current || disposedRef.current) return;
        spawnedRef.current = true;

        let resolveReady: (id: number) => void = () => {};
        let rejectReady: (e: unknown) => void = () => {};
        readyRef.current = new Promise<number>((resolve, reject) => {
            resolveReady = resolve;
            rejectReady = reject;
        });

        invoke<number>("pty_spawn", {
            cols: 80,
            rows: 24,
            cwd: cwd ?? null,
            startup: startup ?? null,
            context: context ?? null,
        }).then(
            (id) => {
                if (disposedRef.current) {
                    void invoke("pty_kill", { id });
                    return;
                }
                pidRef.current = id;
                resolveReady(id);
            },
            (err) => {
                spawnedRef.current = false;
                readyRef.current = null;
                rejectReady(err);
            },
        );
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [spawnWhen]);

    return readyRef;
}
