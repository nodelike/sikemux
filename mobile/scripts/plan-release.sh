#!/usr/bin/env bash
# Plans the phone release of the checked-out commit for the Mobile release workflow, printing its
# outputs as name=value lines.
#   plan-release.sh nightly          main's next nightly, or skip=true when no phone code changed
#                                    since the last phone release. Writes the nightly's version into
#                                    app.json and project.yml and its notes into RELEASE_NOTES.md.
#   plan-release.sh mobile-v<ver>    a pushed tag. A stable one names the nightly it promotes: the
#                                    newest before it whose phone code it shares.
set -euo pipefail

cd "$(dirname "$0")/../.."
PHONE=(mobile shared crates/sikemux-mobile crates/sikemux-wire crates/sikemux-client)
NOTES=mobile/RELEASE_NOTES.md
NOTES_LIMIT=480

app_version() { node -p "require('./mobile/app/app.json').expo.version"; }

version_code() {
  (cd mobile/app && SIKEMUX_MOBILE_VERSION="$1" APP_VARIANT=production \
    node -p "require('./app.config.js')({ config: require('./app.json').expo }).android.versionCode")
}

nightly() {
  local last base major minor patch n version
  last="$(git tag -l 'mobile-v*' --merged HEAD --sort=-creatordate | head -1)"
  if [[ -n "$last" ]] && git diff --quiet "$last" HEAD -- "${PHONE[@]}"; then
    echo "skip=true"
    echo "Nothing the phones build from changed since $last" >&2
    return
  fi

  base="$(app_version)"
  if git rev-parse -q --verify "refs/tags/mobile-v$base" >/dev/null; then
    IFS=. read -r major minor patch <<<"$base"
    sed -i.bak "s/\"version\": \"$base\"/\"version\": \"$major.$minor.$((patch + 1))\"/" mobile/app/app.json
    sed -i.bak "s/^\( *MARKETING_VERSION: *\)$base\$/\1$major.$minor.$((patch + 1))/" mobile/ios/project.yml
    rm -f mobile/app/app.json.bak mobile/ios/project.yml.bak
    base="$major.$minor.$((patch + 1))"
  fi
  n="$(git tag -l "mobile-v$base-nightly.*" | sed 's/.*-nightly\.//' | sort -n | tail -1)"
  version="$base-nightly.$((${n:-0} + 1))"

  {
    echo "# $version"
    {
      git log --reverse --no-merges --format=%s ${last:+"$last..HEAD"} -- "${PHONE[@]}" |
        grep -E '^(feat|fix|perf)(\([^)]*\))?!?: ' |
        sed -E 's/^[a-z]+(\([^)]*\))?!?: /• /; s/^(• [^.]*\.) .*$/\1/; s/\*\*//g; s/\]\(/] (/g' || true
    } | awk -v max="$NOTES_LIMIT" '{ if (total + length($0) + 1 > max) next; total += length($0) + 1; print }' |
      grep . || echo "• Fixes and improvements."
  } >"$NOTES"

  echo "version=$version"
  echo "channel=nightly"
  echo "build=$(version_code "$version")"
}

tagged() {
  local tag="$1" version base channel promoted
  version="${tag#mobile-v}"
  base="$(app_version)"
  if [[ "${version%%-*}" != "$base" ]]; then
    echo "::error::Tag $tag is not a release of $base, the version in mobile/app/app.json" >&2
    exit 1
  fi
  case "$version" in
    *-nightly.*) channel=nightly ;;
    *-*) echo "::error::$version is neither a nightly nor a stable version" >&2; exit 1 ;;
    *) channel=stable ;;
  esac
  echo "version=$version"
  echo "channel=$channel"
  echo "build=$(version_code "$version")"
  if [[ "$channel" == stable ]]; then
    promoted="$(git tag -l "mobile-v$version-nightly.*" --merged HEAD --sort=-v:refname | head -1)"
    if [[ -z "$promoted" ]] || ! git diff --quiet "$promoted" HEAD -- "${PHONE[@]}" ":(exclude)$NOTES"; then
      echo "::error::$tag must be on a commit whose phone code a $version nightly was built from; only $NOTES may differ" >&2
      exit 1
    fi
    echo "promoted=${promoted#mobile-v}"
    echo "promoted_build=$(version_code "${promoted#mobile-v}")"
  fi
}

ios_version="$(sed -n 's/^ *MARKETING_VERSION: *//p' mobile/ios/project.yml)"
[[ "$ios_version" == "$(app_version)" ]] || {
  echo "::error::mobile/ios/project.yml is at $ios_version and mobile/app/app.json at $(app_version): release both at one version" >&2
  exit 1
}

case "${1:-}" in
  nightly) nightly ;;
  mobile-v*) tagged "$1" ;;
  *) echo "Usage: $0 nightly|mobile-v<version>" >&2; exit 2 ;;
esac
