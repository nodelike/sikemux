import { chat } from "./perf/chat";
import { git } from "./perf/git";
import { startup } from "./perf/startup";
import { terminal } from "./perf/terminal";
import { theme } from "./perf/theme";

/*
 * Times the app in a WebKit tab: open /showcase/perf.html and call
 * `perf.run("git")`, or any other scenario here. Serve it built for
 * production, since React's development build is several times slower.
 */

const SCENARIOS: Record<string, (options?: never) => Promise<object>> = {
  chat,
  git,
  startup,
  terminal,
  theme,
};

async function run(name: string, options?: object) {
  const scenario = SCENARIOS[name];
  if (!scenario) throw new Error(`no perf scenario "${name}"`);
  const result = await scenario(options as never);
  console.log(result);
  return result;
}

Object.assign(window, { perf: { run } });
