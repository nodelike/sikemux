import { Compartment, type Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { Theme } from "../themes";
import { currentTheme, setEditorThemeBridge } from "../themes/bus";
import { buildEditorThemeExtensions, indentMarkerExtensions } from "./themeExtensions";

const themeCompartment = new Compartment();
const darkTheme = buildEditorThemeExtensions(true);
const lightTheme = buildEditorThemeExtensions(false);

let editorTheme: Extension = currentTheme().dark ? darkTheme : lightTheme;

export function themeCompartmentExtension(opts: { indentMarkers?: boolean } = {}): Extension {
    return [themeCompartment.of(editorTheme), ...(opts.indentMarkers === false ? [] : [indentMarkerExtensions])];
}

function pushThemeOnto(view: EditorView): void {
    if (themeCompartment.get(view.state) !== editorTheme) view.dispatch({ effects: themeCompartment.reconfigure(editorTheme) });
}

function rebuild(theme: Theme): void {
    editorTheme = theme.dark ? darkTheme : lightTheme;
}

// The theme bus holds every live editor but must not drag CodeMirror into the
// boot bundle, so this module hands it the two things it cannot do itself.
setEditorThemeBridge({ rebuild, push: pushThemeOnto });
