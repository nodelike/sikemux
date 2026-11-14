---
name: sikemux
description: Sikemux's tools through the `sikemux tool` command, the route to use first for Jira, Vercel, Rundeck, AWS, SigNoz, other agents, waiting on processes, the desk browser, the simulator, pages and git_commit. Load it before any of those, before reaching for another MCP, CLI or your own agent tools.
---

# Sikemux tools

Every tool Sikemux's instructions name, such as `jira`, `browser_navigate`, `agent_send` or `git_commit`, is a subcommand of `sikemux tool`, run from your shell.

```sh
sikemux tool                          # every tool, by family; read all of it, never through head
sikemux tool help jira                # what it does and its exact arguments
sikemux tool jira '{"action": "search", "jql": "assignee = currentUser()"}'
```

## When to use it

`sikemux tool` comes first, before any other route to the same thing:

- Jira, Vercel, Rundeck, AWS, SigNoz and the other service tools carry the person's sign-in. Use them, never an Atlassian or other MCP, the `vercel`, `aws` or `gh` CLI, or a raw API, even when one is in your own tool list.
- Other agents (named like Otter): `agent_list`, `agent_send`, `agent_read`, `agent_spawn`, `who_wrote`. Never your own SendMessage, ListAgents or Task for them.
- Waiting: `process_start` and `process_read` for commands the person may watch, `trigger_add` to be woken later, `events_wait` or `agent_wait` for a short wait (after `process_start`, `events_wait` with its `executionId` and `until: "ready"` or `"exit"`). Never `sleep`, a polling loop, or `sleep` between two calls.
- Pages, the desk browser, the simulator, commits: the `page_*`, `browser_*`, `sim` and `git_commit` tools, not curl, a headless browser, `simctl` or `git commit`.

## How to call it

- Run `sikemux tool help NAME` before your first call to each tool. Field names are exact, and a guessed one is refused.
- Arguments are one JSON object in single quotes. For long or quoted text, pass `-` and send the JSON on stdin with a quoted heredoc (`<<'EOF'`).
- The answer prints to stdout. A refusal exits non-zero with the reason: do what it says, never get around it.
- Run calls one after another, each as soon as the last answers; never in the background with `&`.
- A picture, such as a screenshot, comes back as `image saved to PATH`. Read that file to see it.
- `sikemux tool guide` lists the guide's topics; `sikemux tool guide '{"topic": "agents"}'` reads one. Read it before your first process, browser or agent call.
- `sikemux tool instructions` repeats how Sikemux expects you to work.
