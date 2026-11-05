#!/usr/bin/env bash
# What in a commit message credits Claude, read by the commit hook, the push hook
# and CI's pull request check. Commits here credit only people.
# shellcheck disable=SC2034
CLAUDE_CREDIT='^co-authored-by:.*(claude|anthropic[.]com)|generated with [[]?claude code'
