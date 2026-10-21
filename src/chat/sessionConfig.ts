import { recordOf } from "../lib/guards";
import { chatDriver } from "../agents/catalog";
import type { Agent } from "../state/types";

/** One value a session option can take. */
export interface ConfigChoice {
    value: string;
    label: string;
    description?: string;
}

export interface SessionConfig {
    id: string;
    name: string;
    category?: string;
    currentValue: string;
    options: ConfigChoice[];
}

/** The option that sets how hard the model thinks. Agents name it differently
    but tag it with the same category. */
export function effortConfig(configs: SessionConfig[]): SessionConfig | undefined {
    return configs.find((config) => config.category === "thought_level");
}

const EFFORT_ON_EVERY_MODEL = new Set(["claude", "codex"]);

const NAMED_VERSION = /^(\p{L}+)\s+(\d+(?:\.\d+)?)\b/u;

// The agent names a model without its release number ("Opus") and leaves that
// number in the description ("Opus 5 with 1M context"), so put it back.
function versioned(label: string, description?: string): string {
    const named = description?.match(NAMED_VERSION);
    if (!named) return label;
    const [, family, version] = named;
    const head = label.split(" ")[0];
    return head.toLowerCase() !== family.toLowerCase() || label.includes(version) ? label : label.replace(head, `${head} ${version}`);
}

function choices(value: unknown, group?: string): ConfigChoice[] {
    if (!Array.isArray(value)) return [];
    return value.flatMap((item): ConfigChoice[] => {
        const row = recordOf(item);
        if (!row) return [];
        if (Array.isArray(row.options)) return choices(row.options, typeof row.name === "string" ? row.name : undefined);
        if (typeof row.value !== "string" || typeof row.name !== "string") return [];
        const description = typeof row.description === "string" ? row.description : group;
        return [{ value: row.value, label: versioned(row.name, description), description }];
    });
}

/** The name of the value an option holds, or the value itself when its choices do not list it. */
export function currentLabel(config: SessionConfig | undefined): string | undefined {
    return config && (config.options.find((option) => option.value === config.currentValue)?.label ?? config.currentValue);
}

export function sessionConfigs(setup: Record<string, unknown>): SessionConfig[] {
    if (!Array.isArray(setup.configOptions)) return [];
    return setup.configOptions.flatMap((value): SessionConfig[] => {
        const row = recordOf(value);
        if (!row || row.type !== "select" || typeof row.id !== "string" || typeof row.currentValue !== "string") return [];
        return [
            {
                id: row.id,
                name: typeof row.name === "string" ? row.name : row.id,
                ...(typeof row.category === "string" ? { category: row.category } : {}),
                currentValue: row.currentValue,
                options: choices(row.options),
            },
        ];
    });
}

/* Agents on the Claude and Codex drivers always show both pickers, disabled
   until the session fills them. The others only offer effort for some models,
   so theirs comes and goes with the model. */
export function pickerSlots(configs: SessionConfig[], type: Agent["type"]): { id: string; config?: SessionConfig }[] {
    const slots: { id: string; config?: SessionConfig }[] = [{ id: "model", config: configs.find((item) => item.id === "model") }];
    const effort = effortConfig(configs);
    if (effort || EFFORT_ON_EVERY_MODEL.has(chatDriver(type) ?? "")) slots.push({ id: effort?.id ?? "effort", config: effort });
    return slots;
}
