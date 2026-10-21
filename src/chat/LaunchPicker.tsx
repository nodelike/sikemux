import { useEffect, useState } from "react";
import { agentLabel, type ChatAgentType } from "../agents/catalog";
import { coreCall } from "../api/core";
import { useStore } from "../state/store";
import { swallow } from "../state/toast";
import { AgentIcon } from "../ui/Icons";
import { PayerChoices, Picker, servedLabel, useRoutes } from "./ComposerPickers";
import type { Choice } from "./PickerMenu";
import { sessionConfigs } from "./sessionConfig";
import "../styles/chat/pickers.css";

/** An agent to start later: which one, which of the person's accounts, who pays, and on what model. */
export interface LaunchChoice {
    type: ChatAgentType;
    profileId?: string;
    provider: string | null;
    model: string | null;
}

/** The models `type` offered when a chat of it last started, since none runs here to ask. */
function useOfferedModels(type: ChatAgentType | null): Choice[] {
    const [models, setModels] = useState<Choice[]>([]);
    useEffect(() => {
        if (!type) return setModels([]);
        let current = true;
        void coreCall<unknown>("agentModels", { agent: type })
            .then((offered) => {
                if (current) setModels(offered ? (sessionConfigs({ configOptions: [offered] })[0]?.options ?? []) : []);
            })
            .catch(swallow("the agent's models"));
        return () => {
            current = false;
        };
    }, [type]);
    return models;
}

/** The composer's agent and model picker, for choosing an agent before any chat of it runs. */
export function LaunchPicker({
    name,
    value,
    any,
    onChange,
    disabled = false,
}: {
    name: string;
    value: LaunchChoice | null;
    /** What a null value means, offered before the agents. */
    any: string;
    onChange: (next: LaunchChoice | null) => void;
    disabled?: boolean;
}) {
    const profiles = useStore((s) => s.providerProfiles);
    const type = value?.type ?? null;
    const profile = profiles.find((item) => item.id === value?.profileId);
    const [routes, loadRoutes] = useRoutes(type ?? "claude", value?.provider);
    const route = routes.find((item) => item.provider.id === value?.provider);
    const offered = useOfferedModels(route ? null : type);
    const icon = type ? <AgentIcon type={type} size={15} className={`agent-glyph ${type}`} /> : undefined;
    const models: Choice[] = !type
        ? []
        : [
              { value: "", label: "Its default model", icon },
              ...(route ? route.served.models.map((model) => ({ value: model, label: servedLabel(route, model) })) : offered).map((option) => ({
                  ...option,
                  icon,
              })),
          ];
    const modelName = value?.model ? (models.find((option) => option.value === value.model)?.label ?? value.model) : null;
    const label = !value ? any : [profile?.name ?? agentLabel(value.type), modelName, route?.provider.label].filter(Boolean).join(" · ");

    return (
        <Picker
            field
            compact
            empty="Pick an agent to choose its model."
            name={name}
            label={label}
            value={value?.model ?? ""}
            options={models}
            disabled={disabled}
            icon={icon}
            agents={{
                type: type ?? "claude",
                profile,
                any: { label: any, chosen: !value, onPick: () => onChange(null) },
                onSwitch: (next, profileId) => onChange({ type: next, profileId, provider: null, model: null }),
            }}
            payers={
                value && routes.length > 0 ? (
                    <PayerChoices
                        type={value.type}
                        modelProvider={value.provider}
                        profile={profile}
                        routes={routes}
                        onPick={(next) =>
                            onChange({ ...value, provider: next?.provider.id ?? null, model: next?.served.defaults[value.type] ?? null })
                        }
                    />
                ) : undefined
            }
            onOpen={loadRoutes}
            onSelect={(model) => value && onChange({ ...value, model: model || null })}
        />
    );
}
