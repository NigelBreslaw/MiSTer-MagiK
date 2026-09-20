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

shared_project_environment() {
  local repository="$1"
  local project="$2"
  shift 2

  local primary_checkout primary_project local_project dependency_file environment
  primary_checkout="$(shared_primary_checkout "$repository")" || return 1
  [[ "$primary_checkout" != "$repository" ]] || return 1

  local_project="$repository/$project"
  primary_project="$primary_checkout/$project"
  for dependency_file in "$@"; do
    [[ -f "$local_project/$dependency_file" ]] || return 1
    [[ -f "$primary_project/$dependency_file" ]] || return 1
    cmp -s \
      "$local_project/$dependency_file" \
      "$primary_project/$dependency_file" || return 1
  done

  environment="$primary_project/.venv"
  [[ -x "$environment/bin/python" ]] || return 1
  printf '%s\n' "$environment"
}
