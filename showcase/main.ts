import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import "./showcase.css";
import { PANE_IMAGE } from "./world/projects";

mockWindows("main");
Object.assign(
  (window as unknown as { __SHOWCASE_TAURI_INTERNALS__: object })
    .__SHOWCASE_TAURI_INTERNALS__,
  {
    convertFileSrc: (path: string, protocol: string) =>
      `/__showcase/${protocol}/${encodeURIComponent(path)}`,
  },
);
mockIPC(
  (command) => {
    if (command === "plugin:app|version") return "0.4.2";
    return null;
  },
  { shouldMockEvents: true },
);

const { installIpcTransportForTests } = await import("../src/api/transport");
const { ShowcaseBackend } = await import("./backend");
const backend = new ShowcaseBackend();
installIpcTransportForTests(backend);

await import("../src/main.tsx");

const [commands, store, gitWorkbench, git] = await Promise.all([
  import("../src/state/commands"),
  import("../src/state/store"),
  import("../src/state/gitWorkbench"),
  import("./world/git"),
]);
store.useStore.setState({ paneImage: PANE_IMAGE });
for (const [repo, draft] of Object.entries(git.COMMIT_DRAFT)) {
  gitWorkbench.setGitDraft(repo, draft);
}
Object.assign(window, {
  showcase: { backend, cmd: commands, store: store.useStore },
});

// Captures step agent turns frame by frame; an interactive page plays them in real time.
if (new URLSearchParams(location.search).has("play")) {
  const play = () => {
    const hold = backend.stepLive();
    setTimeout(play, hold < 0 ? 250 : hold);
  };
  play();
}
