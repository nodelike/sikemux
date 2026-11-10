#!/usr/bin/env bash
# Prints the open, non-draft pull requests the person picked, each number and head commit as it
# first appears, newest last, so a watcher fires once when one is picked or gets new commits.
# Picked: approved on GitHub by the owner, started from Sikemux's Pull requests page, or named
# by its address in a prompt the person typed to an agent in the last two weeks.
# When GitHub can't be reached it prints the last answer, so an outage doesn't look like a change.
set -euo pipefail

repo=nodelike/sikemux
owner=nodelike
data="$HOME/Library/Application Support/com.nodelike.sikemux"
seen="${TMPDIR:-/tmp}/sikemux-pr-fan-seen"
touch "$seen"

if ! open=$(gh pr list -R "$repo" --state open --limit 100 --json number,headRefOid,isDraft,reviews \
  --jq ".[] | select(.isDraft | not) | \"\(.number) \(.headRefOid[0:12]) \(any(.reviews[]; .author.login == \"$owner\" and .state == \"APPROVED\"))\"" 2>/dev/null); then
  tail -n 20 "$seen"
  exit 0
fi

url="github\\.com/$repo/pull/([0-9]+)"
prompted="${TMPDIR:-/tmp}/sikemux-pr-fan-prompted"
since=(-mtime -14)
[[ -f "$prompted" ]] && since=(-newer "$prompted")
found=$(find "$HOME"/.claude*/projects/-Users-nodelike-proj-pers-sikemux* -name '*.jsonl' "${since[@]}" -print0 2>/dev/null |
  xargs -0 rg --no-filename -oN "\"role\":\"user\",\"content\":\[\{\"type\":\"text\",\"text\":\"[^<\"][^\"]*$url" 2>/dev/null |
  grep -oE "$url" | grep -oE '[0-9]+$' || true)
printf '%s\n' "$found" >>"$prompted"
picked=$(
  {
    cat "$prompted"
    cat "$data"/boards/*.json "$data"/board-archive/*.json 2>/dev/null |
      jq -r '.cards[]? | .details // empty' 2>/dev/null |
      grep -oE "^(Review|Answer the review comments on) pull request #[0-9]+ .*$url" |
      grep -oE "$url" | grep -oE '[0-9]+$' || true
  } | sort -u
)

while read -r number sha approved; do
  [[ -n "$number" ]] || continue
  [[ "$approved" == true ]] || grep -qx "$number" <<<"$picked" || continue
  line="#$number $sha"
  grep -qxF "$line" "$seen" || printf '%s\n' "$line" >>"$seen"
done <<<"$open"
tail -n 20 "$seen"
