import { useState } from "react";
import { SendToAgentMenu } from "../../../plugin-api/ui";
import type { DatabaseResultSet } from "../types.generated";

/** A button that hands these results, and the SQL behind them, to one of the project's agents. */
export function SendResults({ result }: { result: DatabaseResultSet }) {
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    return (
        <>
            <button
                type="button"
                className="db-button"
                title="Send these results and the SQL to an agent"
                onClick={(event) => {
                    const box = event.currentTarget.getBoundingClientRect();
                    setMenu({ x: box.right, y: box.bottom + 4 });
                }}>
                Send to agent
            </button>
            {menu && <SendToAgentMenu x={menu.x} y={menu.y} alignRight delivery={() => ({ text: result.forAgent })} onClose={() => setMenu(null)} />}
        </>
    );
}
