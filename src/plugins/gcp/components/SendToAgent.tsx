import { useState } from "react";
import { SendToAgentMenu } from "../../../plugin-api/ui";

/** A button that hands an agent the text `delivery` makes when the person picks one. */
export function SendToAgent({ delivery, disabled }: { delivery: () => string; disabled?: boolean }) {
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    return (
        <>
            <button type="button" className="gcp-button" disabled={disabled} onClick={(event) => setMenu({ x: event.clientX, y: event.clientY })}>
                Send to agent
            </button>
            {menu && <SendToAgentMenu x={menu.x} y={menu.y} alignRight delivery={() => ({ text: delivery() })} onClose={() => setMenu(null)} />}
        </>
    );
}
