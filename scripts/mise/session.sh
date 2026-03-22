#!/usr/bin/env bash
if [ $# -lt 2 ]; then
    echo "Usage: mise run session -- <name> <goal> [project-dir]"
    echo ""
    echo "Examples:"
    echo '  mise run session -- auth-refactor "refactoring auth middleware" ~/myproject'
    echo '  mise run session -- frontend "building the dashboard" .'
    exit 1
fi
./scripts/session-start.sh "$@"
