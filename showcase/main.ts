import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import "./showcase.css";

mockWindows("main");
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
for (const [repo, draft] of Object.entries(git.COMMIT_DRAFT)) {
  gitWorkbench.setGitDraft(repo, draft);
}
Object.assign(window, {
  showcase: { backend, cmd: commands, store: store.useStore },
});
