#!/bin/bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

shared_primary_checkout() {
  local repository="$1"
  local common_git_dir primary_checkout
  if ! common_git_dir="$(git -C "$repository" rev-parse --path-format=absolute --git-common-dir)"; then
    echo "cannot resolve the shared Git common directory for $repository" >&2
    return 1
  fi
  case "$common_git_dir" in
    */.git) primary_checkout="${common_git_dir%/.git}" ;;
    *)
      echo "cannot derive the primary checkout from Git common directory $common_git_dir" >&2
      return 1
      ;;
  esac
  if [[ -z "$primary_checkout" || "$primary_checkout" == / || ! -d "$primary_checkout" ]]; then
    echo "unsafe primary checkout derived from Git common directory $common_git_dir" >&2
    return 1
  fi
  printf '%s\n' "$primary_checkout"
}
