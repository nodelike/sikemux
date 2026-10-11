import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Checkbox, Slider } from "./Controls";

afterEach(cleanup);

describe("Controls", () => {
    it("keeps the checkbox label clickable", async () => {
        const user = userEvent.setup();
        const onChange = vi.fn();
        render(
            <Checkbox checked={false} onChange={onChange}>
                project
            </Checkbox>,
        );

        await user.click(screen.getByText("project"));
        expect(onChange).toHaveBeenCalledWith(true);
    });

    it("reports slider changes as numbers and paints the filled ratio", () => {
        const onChange = vi.fn();
        const { container } = render(<Slider value={15} min={0} max={60} onChange={onChange} label="Background blur" format={(v) => `${v}px`} />);

        expect(screen.getByText("15px")).toBeInTheDocument();
        expect(container.querySelector(".sld")).toHaveStyle({ "--sld-fill": "25%" });

        fireEvent.change(screen.getByRole("slider", { name: "Background blur" }), { target: { value: "30" } });
        expect(onChange).toHaveBeenCalledWith(30);
    });
});
