# Tool-use eval

Real failures from agent transcripts, replayed against sikemux-tools to measure how well each model picks and calls its tools. It is run by hand and stays out of `pnpm check` and CI.

```sh
pnpm eval:tools --cases 'browser-*' --models sonnet
pnpm eval:tools --models sonnet,opus,deepseek --search both --record baseline
pnpm eval:tools --list
pnpm eval:tools --at 1dacdf57d --models sonnet   # the tools as they were at an earlier commit
```

DeepSeek V4.1 Flash runs through OMP on LithosAI, as Sikemux runs it; it needs `TOOL_EVAL_LITHOS_KEY_FILE`, the path of a file holding the key. `--at` needs the host built at that commit, which every run on a clean checkout saves under `/tmp/sikemux-tool-eval/hosts/`.

Each case runs the agent CLI (`claude -p`, or OMP for DeepSeek) in a temporary home and a fresh fixture repo, inside `sandbox-exec`, so it cannot reach the real desk, browser, git, simulator or the person's files. Nothing in that sandbox can run or read a Sikemux program, use the keychain or `launchctl`, signal a process outside it, or reach Sikemux's sockets: a Sikemux core started with the sandbox's home once replaced the person's host key. Claude's sign-in is handed over in the environment, it can reach no local port but its tools' relay, and every process a case started is killed when it ends. Its zsh gets the options Sikemux gives agents' shells, read from `agent_shell.rs`, and a GNU-style `timeout`. Its tools come from the real `sikemux-host --tools-mcp` built from this checkout, which the harness runs outside the agent's sandbox behind a relay, talking to a fake Sikemux (`lib/core.mjs`) that serves `tools/tools.json`, the guide and the plugin manifests straight from the repo and answers each call from the case's fixture. `page_preview` really draws the page in an offscreen WebKit view (`lib/preview.swift`, compiled on first use) with Sikemux's base styles and a dark theme, inside the agent's sandbox, running its `script` and measuring its height as the real tool does. `browser_screenshot` draws the fixture page the same way, from its `html` or its elements and text, at the tab's viewport. A description change in the repo shows up in the next run. Rival routes the cases need, such as an Atlassian MCP, come from `rivals/`, and commands like `gh`, `sleep` and `xcrun` from shims in `lib/shims.mjs`.

## Cases

`cases/*.json` hold arrays of cases:

- `prompt`: what the agent is told.
- `fixture`: `project` (`sikemux` or `work`), `files`, `commits`, `dirty`, `remote` and `branches` for the repo, `yours` for a file as git_commit would commit it when other agents' uncommitted lines share it, `instructions` added to its AGENTS.md, `tab`, `tabs` and `pages` (each with `url`, `title`, `elements`, `text` and optional `html`) for the desk browser, `agents` beside it, `answers` (rules of `tool`, optional `args`, `reply` or `error` where `{{arg}}` stands for an argument of the call, optional `times`) for any tool, `shell` rules for shimmed commands, `rivals` and `plugins`.
- `grade`: `expect` (calls that must happen, in `order` if set), `forbid`, `limit`, `first`, `reply`, `files` (what the repo must hold afterwards: `path` with `match`, or `absent`), and `allow` to lift one of the checks every case gets: no Atlassian MCP, no Claude browser, no SendMessage, no raw `git commit` or `git add -A`, no sleeping, and no two calls at once on the simulator or on a file being written. Browser calls batched on one tab are fine: the server runs them in order.
- `judge`: a rubric for a judge model, for what calls alone cannot show.
- `skip`: why a case cannot be replayed.

A call matcher is `tool` (a name, a `prefix*` or a list) with optional `args`, or `any` of several matchers. Argument matchers compare values, `"/regex/flags"` strings, `{"$present": bool}`, `{"$not": …}` and `{"$any": […]}`, which also checks any keys beside it. A tool that takes an `action`, such as `browser`, is matched as `{"tool": "browser", "args": {"action": "click"}}`.

Runs land in `/tmp/sikemux-tool-eval/<time>/`, one folder per case with its transcript and every call the fake Sikemux answered; each case's home and repo are deleted once it is graded unless `--keep` is given. `--resume <run folder>` finishes a run that stopped, grading what finished and running the rest. `--shell` and `--shell-env` change the shell agents' commands run in. `--trials N` runs every case N times and reports the mean pass rate beside the cases that passed every trial. `--record <label>` also saves the result under `results/`, or under `--results-dir`. `--cases-dir` runs cases kept in another folder.

## Saved results and the queue

Every run of a case is saved in `~/Library/Caches/sikemux-tool-eval/results/`, keyed by a hash of the case, the tools as the agent meets them (`tools/`, the `tools_mcp` source with the server's instructions, the plugin manifests, `AGENTS.md`, the system prompt lines), the harness, the Claude CLI's version, the limits, the model and the trial number. A rerun with the same inputs reuses the result, marks it `(saved)` and spends nothing; `--fresh` runs it again. Runs that timed out or failed to start are not saved. A full run records each case's inputs, and `--affected` then runs only the cases whose inputs changed since.

One eval runs at a time on a machine: a second waits in line and says whose run it is behind.

Iterate on one trial of a `--cases` subset, and make one full `--trials 3` Sonnet run per change, never two on one commit.
