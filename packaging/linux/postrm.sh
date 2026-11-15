#!/bin/sh
# Forgets Sikemux's package repository once the package is gone, since its
# source names a key file that went with it.
set -e
case "$1" in
  remove | purge) rm -f /etc/apt/sources.list.d/sikemux.sources ;;
esac
