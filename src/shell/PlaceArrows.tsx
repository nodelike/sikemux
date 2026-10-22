import { useSyncExternalStore } from "react";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import * as cmd from "../state/commands";
import { IconChevron } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";

/** Back and forward through the projects, tabs, panes and files the person was in, as a code editor's title bar has them. */
export function PlaceArrows() {
    const history = useSyncExternalStore(cmd.subscribePlaceHistory, cmd.placeHistory);
    const backShortcut = useShortcutLabel("place.back");
    const forwardShortcut = useShortcutLabel("place.forward");
    return (
        <div className="place-arrows">
            <Tooltip label={withShortcut("Go back", backShortcut)}>
                <button type="button" className="tb-btn" aria-label="Go back" disabled={!history.canGoBack} onClick={cmd.goBack}>
                    <IconChevron size={13} className="place-back-icon" />
                </button>
            </Tooltip>
            <Tooltip label={withShortcut("Go forward", forwardShortcut)}>
                <button type="button" className="tb-btn" aria-label="Go forward" disabled={!history.canGoForward} onClick={cmd.goForward}>
                    <IconChevron size={13} />
                </button>
            </Tooltip>
        </div>
    );
}
