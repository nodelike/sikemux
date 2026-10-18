import { invokeCommand as invoke } from "./invoke";

/** `api` bills per token, `plan` is a monthly coding subscription, `local` runs on this computer. */
export type ModelProviderKind = "api" | "plan" | "local";

/** What models.dev says of a model. */
export interface ModelFacts {
    context: number;
    /** USD per million tokens. */
    input?: number;
    output?: number;
    /** Only a hint until a check runs. */
    toolCall: boolean;
}

/** What a provider starts chats on, and what models.dev says of those models, by API name. */
export interface ModelChoices {
    /** The coding model helpers and checks start on. */
    model?: string;
    /** The model each agent's chat starts on. */
    defaults: Record<string, string>;
    facts: Record<string, ModelFacts>;
}

/** One entry of `src-tauri/crates/sikemux-core/model-providers.json`, with what Sikemux knows of it now. */
export interface ModelProvider extends ModelChoices {
    id: string;
    label: string;
    kind: ModelProviderKind;
    /** Where the person makes a key, or downloads a local runner. */
    signUpUrl: string;
    /** Base URL of its OpenAI-compatible API. */
    openai: string;
    /** A keyed provider has its key saved; a local runner is answering. */
    connected: boolean;
    /** What a running local runner has installed. */
    models?: string[];
    /** The agents a chat can run on it in place of their makers' accounts. */
    agents: string[];
    /** Whether each model checked so far calls tools the way agents need. */
    checks: Record<string, ModelCheck>;
}

export interface ModelCheck {
    works: boolean;
    /** Unix milliseconds. */
    at: number;
    detail?: string;
}

/** What helpers run on: workers started without a model named, and Claude Code's subagents. */
export interface ModelHelper {
    agent: string;
    provider?: string;
    model?: string;
}

export interface ServedModels extends ModelChoices {
    models: string[];
}

/** Checks and facts are kept by the model's API name, without Claude Code's long-context suffix. */
const apiName = (model: string) => model.replace(/\[1m\]$/, "");

export const checkWords = (check: ModelCheck): string => (check.works ? "works with agents" : "tool calls failed");

const tokens = new Intl.NumberFormat("en", { notation: "compact" });

/** Context, price per million tokens and tool calls, in a few words. A check outranks what models.dev says. */
export function modelWords(choices: ModelChoices, checks: Record<string, ModelCheck>, model: string): string[] {
    const name = apiName(model);
    const facts = choices.facts[name];
    const check = checks[name];
    const words = facts ? [`${tokens.format(facts.context)} context`] : [];
    if (facts?.input && facts.output) words.push(`$${facts.input}/$${facts.output} per 1M`);
    if (check) words.push(checkWords(check));
    else if (facts) words.push(facts.toolCall ? "lists tool calls" : "no tool calls");
    return words;
}

export const modelProvidersApi = {
    list: () => invoke<ModelProvider[]>("model_providers"),
    /** What a connected keyed provider serves, read with its key. */
    models: (id: string) => invoke<ServedModels>("model_provider_models", { id }),
    connect: (id: string, key: string) => invoke<void>("model_provider_connect", { id, key }),
    disconnect: (id: string) => invoke<void>("model_provider_disconnect", { id }),
    /** Asks the provider now whether the model calls tools the way agents need. */
    check: (id: string, model: string) => invoke<ModelCheck>("model_provider_check", { id, model }),
    helper: () => invoke<ModelHelper | null>("model_helper"),
    setHelper: (helper: ModelHelper | null) => invoke<void>("model_helper_set", { helper }),
};
