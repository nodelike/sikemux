import { invokeCommand as invoke } from "./invoke";

export type ListeningPortOwner =
    | {
          kind: "pty";
          ptyId: number;
          project: string | null;
          paneId: string | null;
          agentId: string | null;
          taskExecutionId: string | null;
      }
    | { kind: "agent"; agentId: string };

export interface ListeningPort {
    port: number;
    address: string;
    pid: number;
    process: string;
    owner: ListeningPortOwner;
}

export const portsApi = {
    listening: () => invoke<ListeningPort[]>("listening_ports"),
    /** The ports any program on this Mac already listens on, out of those given. */
    inUse: (ports: number[]) => invoke<number[]>("ports_in_use", { ports }),
    openExternal: (url: string) => invoke<void>("open_url", { url, app: null, shortcut: null }),
};
