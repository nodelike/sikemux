import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useRailDock } from "./railMotion";

function Shell({ visible, present = true, drawn = true }: { visible: boolean; present?: boolean; drawn?: boolean }) {
    useRailDock(visible, present, ".side-rail", drawn);
    if (!drawn) return null;
    return (
        <div className="shell">
            <div className="body">{present && <aside className="side-rail" data-testid="rail" />}</div>
        </div>
    );
}

describe("useRailDock", () => {
    afterEach(cleanup);

    it("keeps a hidden rail mounted and shows the same one again", () => {
        const view = render(<Shell visible />);
        const rail = view.getByTestId("rail");
        expect(rail.style.display).toBe("");

        view.rerender(<Shell visible={false} />);
        expect(view.getByTestId("rail")).toBe(rail);
        expect(rail.style.display).toBe("none");

        view.rerender(<Shell visible />);
        expect(view.getByTestId("rail")).toBe(rail);
        expect(rail.style.display).toBe("");
    });

    it("hides a rail that mounts while its side is closed", () => {
        const view = render(<Shell visible={false} present={false} />);
        view.rerender(<Shell visible={false} />);
        expect(view.getByTestId("rail").style.display).toBe("none");
    });

    it("hides a closed rail once the window is drawn after starting up", () => {
        const view = render(<Shell visible={false} present={false} drawn={false} />);
        view.rerender(<Shell visible={false} drawn={false} />);
        view.rerender(<Shell visible={false} />);
        expect(view.getByTestId("rail").style.display).toBe("none");
    });
});
