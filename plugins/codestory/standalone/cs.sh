#!/bin/sh
# Managed by codestory-cli setup.
case "${1-}" in
  find|refs|read|packet)
    project=$(git rev-parse --show-toplevel 2>/dev/null) || project=$PWD
    cd -- "$project" || exit
    if [ "$1" = packet ]; then
      shift
      exec @CODESTORY_CLI@ packet --launcher-read-only "$@"
    fi
    exec @CODESTORY_CLI@ "$@"
    ;;
  *)
    printf '%s\n' 'Usage: cs find TERM... or --question QUESTION | cs refs TARGET... or --changed | cs read SELECTOR... | cs packet --question QUESTION'
    exit 2
    ;;
esac
