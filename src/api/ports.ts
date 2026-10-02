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
    openExternal: (url: string) => invoke<void>("open_url", { url, app: null, shortcut: null }),
};
