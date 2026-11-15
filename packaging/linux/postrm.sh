#!/bin/sh
# Forgets Sikemux's package repository once the package is gone, since its
# source names a key file that went with it. An rpm passes how many are left.
set -e
case "$1" in
  remove | purge | 0) rm -f /etc/apt/sources.list.d/sikemux.sources /etc/yum.repos.d/sikemux.repo ;;
esac
