import { invokeCommand as invoke } from "./invoke";

/** A hosted model provider OpenCode, Pi and OMP turn on when its key is saved. */
export interface ModelProvider {
    id: string;
    label: string;
    /** Where the person makes a key. */
    keysUrl: string;
    connected: boolean;
}

export const modelProvidersApi = {
    list: () => invoke<ModelProvider[]>("model_providers"),
    connect: (id: string, key: string) => invoke<void>("model_provider_connect", { id, key }),
    disconnect: (id: string) => invoke<void>("model_provider_disconnect", { id }),
};
