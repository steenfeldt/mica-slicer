#!/usr/bin/env bash
# Fetch the pinned upstream sources listed in upstream/UPSTREAM.lock into upstream/<id>/.
# Each checkout is shallow and sits exactly at the pinned commit, so it matches what
# PROVENANCE.md and the "Source:" headers point at.
#
# Usage: scripts/fetch-upstream.sh [--profiles] [id ...]    (no ids = fetch all)
#
# --profiles: only the data the tests read (CI uses this). For orca and snapmaker-orca that is
# a sparse, blob-filtered checkout of resources/profiles (plus Snapmaker Orca's 3DBenchy.3mf),
# a few tens of MB instead of the whole source tree; other ids are fetched whole. A sparse
# checkout is not a source reference: `cargo xtask provenance` does not check paths in it, and a
# later run without --profiles replaces it with the full tree.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
lock="$root/upstream/UPSTREAM.lock"

profiles_only=0
if [ "${1:-}" = "--profiles" ]; then
  profiles_only=1
  shift
fi

# The paths a --profiles checkout keeps (empty: the whole tree).
sparse_paths() {
  case "$1" in
    orca) echo "/resources/profiles/" ;;
    snapmaker-orca) echo "/resources/profiles/ /resources/handy_models/3DBenchy.3mf" ;;
    *) echo "" ;;
  esac
}

want=" $* "
grep -v '^\s*#' "$lock" | grep -v '^\s*$' | while read -r id repo ref commit _license; do
  if [ $# -gt 0 ] && [[ "$want" != *" $id "* ]]; then continue; fi
  dest="$root/upstream/$id"
  paths=""
  if [ "$profiles_only" = 1 ]; then paths="$(sparse_paths "$id")"; fi
  if [ -d "$dest/.git" ] && [ "$(git -C "$dest" rev-parse HEAD)" = "$commit" ]; then
    is_sparse="$(git -C "$dest" config --bool core.sparseCheckout || echo false)"
    # A full checkout serves every request; a sparse one only another --profiles fetch.
    if [ "$is_sparse" != true ] || [ -n "$paths" ]; then
      echo "ok       $id @ ${commit:0:10} ($ref)"
      continue
    fi
  fi
  echo "fetching $id @ ${commit:0:10} ($ref) from $repo${paths:+ (sparse: $paths)}"
  rm -rf "$dest"
  mkdir -p "$dest"
  git -C "$dest" init -q
  # Byte-exact files on every platform (profiles feed the reference hashes).
  git -C "$dest" config core.autocrlf false
  git -C "$dest" remote add origin "$repo"
  if [ -n "$paths" ]; then
    # shellcheck disable=SC2086 # the paths are space-separated patterns
    git -C "$dest" sparse-checkout set --no-cone $paths
    git -C "$dest" fetch -q --depth 1 --filter=blob:none origin "$commit"
  else
    git -C "$dest" fetch -q --depth 1 origin "$commit"
  fi
  git -C "$dest" -c advice.detachedHead=false checkout -q FETCH_HEAD
  actual="$(git -C "$dest" rev-parse HEAD)"
  if [ "$actual" != "$commit" ]; then
    echo "error: $id checked out $actual, expected $commit" >&2
    exit 1
  fi
done
