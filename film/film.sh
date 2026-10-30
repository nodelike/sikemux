#!/usr/bin/env bash
# Films a scene of the GPUI app for posts: builds a dev core, the stand-in Claude and the film
# example, plays film/scenes/<scene>.film offscreen on a throwaway core, then lays the sounds
# (and music, with --music) under it.
#   film/film.sh <scene> [--draft] [--blur] [--music [track]] [--out clip.mp4]
#   film/film.sh <scene> --stills 1.5,4 [--grid]     pictures at those seconds, to film/out/<scene>/
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
GPUI="$ROOT/app"
export PATH="$HOME/.cargo/bin:$PATH"

say() { printf '\033[35m›\033[0m %s\n' "$*"; }
fail() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

scene="${1:-}"
[[ -n "$scene" && -f "$ROOT/film/scenes/$scene.film" ]] ||
  fail "usage: film/film.sh <scene> [...]; scenes: $(cd "$ROOT/film/scenes" && ls *.film | sed 's/\.film$//' | tr '\n' ' ')"
shift

draft=0 blur=0 music="" stills="" grid=0 out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --draft) draft=1; shift ;;
    --blur) blur=1; shift ;;
    --grid) grid=1; shift ;;
    --stills) stills="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --music)
      if [[ -n "${2:-}" && "$2" != --* ]]; then music="$2"; shift 2
      else music="$ROOT/marketing/x/music/launch-bed.m4a"; shift; fi ;;
    *) fail "unknown option: $1" ;;
  esac
done
command -v ffmpeg >/dev/null || fail "ffmpeg is needed (brew install ffmpeg)"

say "building the core, the stand-in Claude and the film"
cargo build --quiet --manifest-path "$ROOT/Cargo.toml" \
  -p sikemux-host -p sikemux-core --bin sikemux-host --bin sikemux-fake-claude
cargo build --quiet --manifest-path "$GPUI/Cargo.toml" --example film
bin="$ROOT/target/debug"

work="$(mktemp -d -t sxfilm)"
trap 'rm -rf "$work"' EXIT
folder="$ROOT/film/out/$scene"
mkdir -p "$folder"

wallpaper="${FILM_WALLPAPER:-$HOME/wallpaper/midnight-garden.jpeg}"
wallpaper_args=()
if [[ -f "$wallpaper" ]]; then
  sips -s format png "$wallpaper" --out "$work/wallpaper.png" >/dev/null
  wallpaper_args=(--wallpaper "$work/wallpaper.png")
fi

args=(
  "$ROOT/film/scenes/$scene.film"
  --core "$bin/sikemux-host" --claude "$bin/sikemux-fake-claude"
  --work "$work/run" --lockup "$ROOT/brand/wordmark/lockup-white-2048.png" --world "$ROOT/film/world.json"
  "${wallpaper_args[@]}"
)
[[ $draft == 1 ]] && args+=(--draft)
[[ $blur == 1 ]] && args+=(--blur)
[[ $grid == 1 ]] && args+=(--grid)

if [[ -n "$stills" ]]; then
  "$bin/examples/film" "${args[@]}" --stills "$stills" --out "$folder"
  exit
fi

if [[ -z "$out" ]]; then
  if [[ $draft == 0 && -d "$ROOT/marketing/x/posts" ]]; then
    out="$ROOT/marketing/x/posts/$scene/clip.mp4"
  else
    out="$folder/clip.mp4"
  fi
fi
mkdir -p "$(dirname "$out")"
silent="$folder/silent.mp4"
events="$folder/events.json"
"$bin/examples/film" "${args[@]}" --out "$silent" --events "$events"

if [[ -d "$ROOT/marketing/x/sfx" ]]; then
  say "laying the sound under it"
  mix=(node "$ROOT/film/mix.mjs" "$silent" "$events" "$out")
  [[ -n "$music" ]] && mix+=(--music "$music")
  "${mix[@]}"
else
  cp "$silent" "$out"
  say "no marketing/x/sfx here, so the clip is silent"
fi
say "wrote $out"
