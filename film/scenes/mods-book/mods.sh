#!/bin/sh
# Puts six mods in the mods folder of the home it runs in.
folder="$HOME/.config/sikemux/mods.dev"
mod() {
  mkdir -p "$folder/$1"
  printf '{ "name": "%s", "icon": "%s", "description": "%s" }\n' "$2" "$3" "$4" >"$folder/$1/mod.json"
  cp page.html "$folder/$1/index.html"
}
mod posts "Posts" share "Each day's drafted posts, to copy and tick."
mod ci-board "CI board" activity "The latest runs of every project."
mod downloads "Downloads" gauge "Downloads of each release."
mod gate "Release gate" rocket "What the next release waits on."
mod reading "Reading list" list-todo "Saved to read."
mod refunds "Refunds queue" "" "Refunds to look at."
