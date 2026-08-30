#!/usr/bin/env sh
set -eu

platform=$(uname -s)
missing=0

case "$platform" in
Linux)
    commands="Xvfb xeyes"
    description="Native runtime"
    ;;
Darwin)
    commands="docker"
    description="macOS OCI runtime"
    ;;
*)
    echo "Unsupported host platform: $platform" >&2
    exit 1
    ;;
esac

for command in $commands; do
    if command -v "$command" >/dev/null 2>&1; then
        printf '%-12s %s\n' "$command" "ok"
    else
        printf '%-12s %s\n' "$command" "missing"
        missing=1
    fi
done

if [ "$missing" -ne 0 ]; then
    echo "Install the missing programs before running the $description." >&2
    exit 1
fi

if [ "$platform" = "Darwin" ]; then
    echo "macOS host dependencies are available. Use an agent-enabled OCI image."
else
    echo "Linux native runtime dependencies are available."
fi
