---
name: sikemux
description: Sikemux's agent tools (desk browser, processes, git_commit, other agents, board, simulator, pages, plugins such as jira and aws) through the `sikemux tool` command. Use it whenever Sikemux's instructions name a tool.
---

# Sikemux tools

Every tool Sikemux's instructions name, such as `browser_navigate`, `git_commit` or `agent_send`, is a subcommand of `sikemux tool`, run from your shell.

```sh
sikemux tool                          # every tool, one line each
sikemux tool help browser_navigate    # what it does and its arguments
sikemux tool browser_navigate '{"url": "http://localhost:3000"}'
```

- Arguments are one JSON object in single quotes. For long or quoted text, pass `-` and send the JSON on stdin with a quoted heredoc (`<<'EOF'`).
- Read `sikemux tool help NAME` before a tool's first call; it is short and has the exact field names.
- The answer prints to stdout. A refusal exits non-zero with the reason: do what it says.
- A picture, such as a screenshot, is saved to a file whose path is printed; Read that file to see it.
- `sikemux tool guide '{"topic": "agents"}'` reads a topic of the guide; with no topic it lists them.
- `sikemux tool instructions` repeats how Sikemux expects you to work.
- Each call is one tool call: run them one after another, not in the background with `&`.
