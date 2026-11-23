#!/usr/bin/env bash
# Prints the newest glibc that the programs and libraries under the given paths ask for, and fails,
# naming each symbol, when that is newer than the oldest glibc the installer accepts: OLDEST_GLIBC
# for the host core, OLDEST_APP_GLIBC for the desktop app.
#   scripts/check-glibc.sh <host|app> <file or folder>...
set -euo pipefail

case "${1:-}" in
  host) key=OLDEST_GLIBC ;;
  app) key=OLDEST_APP_GLIBC ;;
  *)
    echo "usage: $0 <host|app> <file or folder>..." >&2
    exit 2
    ;;
esac
shift
(($#)) || {
  echo "usage: $0 <host|app> <file or folder>..." >&2
  exit 2
}
oldest="$(sed -n "s/^$key=//p" "$(dirname "$0")/install.sh")"
[[ -n "$oldest" ]] || {
  echo "scripts/install.sh names no $key" >&2
  exit 1
}

asked=""
while IFS= read -r -d '' file; do
  [[ "$(head -c 4 "$file")" == $'\x7fELF' ]] || continue
  asked+="$(objdump -T "$file" | awk -v file="$file" '
    { for (i = 1; i <= NF; i++) if ($i ~ /^\(?GLIBC_[0-9]/) {
        v = $i; gsub(/[()]|GLIBC_/, "", v); print v "\t" $NF "\t" file
    } }')"$'\n'
done < <(find "$@" -type f -print0)

[[ -n "${asked//$'\n'/}" ]] || {
  echo "no program under $* asks for glibc" >&2
  exit 1
}
newest="$(cut -f1 <<<"$asked" | sed '/^$/d' | sort -uV | tail -1)"
echo "Newest glibc asked for: $newest (the installer's $key is $oldest)"
too_new="$(awk -F'\t' -v oldest="$oldest" 'NF {
    split($1, have, "."); split(oldest, need, ".")
    if (have[1] > need[1] || (have[1] == need[1] && have[2] > need[2])) print "  " $3 ": " $2 " (GLIBC_" $1 ")"
  }' <<<"$asked" | sort -u)"
[[ -z "$too_new" ]] || {
  echo "These need a glibc newer than $oldest:" >&2
  echo "$too_new" >&2
  exit 1
}
