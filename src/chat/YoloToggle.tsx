import { effectiveMode as permissionModeOf, isYolo } from "@shared/agents/agentLaunch";
import type { Agent } from "../state/types";
import { IconShield, IconShieldBolt } from "@shared/ui/Icons";
import * as cmd from "../state/commands";
import { useShortcutLabel } from "../commands/useShortcutLabel";

export function YoloToggle({ agent, relaunches, disabled = false }: { agent: Agent; relaunches: boolean; disabled?: boolean }) {
    const on = isYolo(permissionModeOf(agent));
    const restart = relaunches ? ", which restarts the CLI" : "";
    const shortcut = useShortcutLabel("agent.permissions");
    const press = shortcut ? `${shortcut} ` : "Click ";
    return (
        <button
            type="button"
            className={`yolo-toggle${on ? " on" : ""}`}
            aria-pressed={on}
            disabled={disabled}
            title={
                on
                    ? `YOLO mode on — ${agent.type} runs without approvals. ${press}turns it off${restart}.`
                    : `Safe mode — ${agent.type} asks before it acts. ${press}goes YOLO${restart}.`
            }
            onClick={() => cmd.toggleAgentSkipPermissions(agent.id)}>
            <span className="yolo-glyph" aria-hidden="true">
                {on ? <IconShieldBolt size={12} /> : <IconShield size={12} />}
            </span>
            <span className="yolo-label">{on ? "yolo" : "safe"}</span>
            {shortcut && (
                <span className="yolo-hint">
                    <kbd>{shortcut}</kbd>
                </span>
            )}
        </button>
    );
}
