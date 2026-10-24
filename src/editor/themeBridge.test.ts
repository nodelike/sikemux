import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { expect, it, vi } from "vitest";
import { SIKEMUX_THEMES, testPalette } from "../test/themes";
import { currentTheme, refreshViewTheme, registerView, wearPicked } from "../themes/bus";
import { themeCompartmentExtension } from "./themeBridge";

it("reuses theme extensions across warm tabs and refreshes a stale tab once", () => {
    const states = Array.from({ length: 20 }, () => EditorState.create({ doc: "text", extensions: themeCompartmentExtension() }));
    const view = new EditorView({ state: states[0] });
    const unregister = registerView(view);
    const dispatch = vi.spyOn(view, "dispatch");
    for (let i = 0; i < 200; i++) {
        view.setState(states[i % states.length]);
        refreshViewTheme(view);
    }
    expect(dispatch).not.toHaveBeenCalled();
    wearPicked(testPalette(SIKEMUX_THEMES.find((theme) => theme.dark !== currentTheme().dark)!));
    refreshViewTheme(view);
    expect(dispatch).toHaveBeenCalledTimes(1);
    refreshViewTheme(view);
    expect(dispatch).toHaveBeenCalledTimes(1);
    unregister();
    view.destroy();
});
