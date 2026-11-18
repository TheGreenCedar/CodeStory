#!/bin/sh
# Managed by codestory-cli setup.
case "${1-}" in
  find|refs|read)
    project=$(git rev-parse --show-toplevel 2>/dev/null) || project=$PWD
    cd -- "$project" || exit
    exec @CODESTORY_CLI@ "$@"
    ;;
  *)
    printf '%s\n' 'Usage: cs find TERM... | cs refs NAME... | cs read SELECTOR...'
    exit 2
    ;;
esac
