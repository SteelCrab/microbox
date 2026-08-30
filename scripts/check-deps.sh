#!/usr/bin/env sh
set -eu

missing=0

for command in Xvfb xeyes; do
    if command -v "$command" >/dev/null 2>&1; then
        printf '%-12s %s\n' "$command" "ok"
    else
        printf '%-12s %s\n' "$command" "missing"
        missing=1
    fi
done

if [ "$missing" -ne 0 ]; then
    echo "Install the missing X11 programs before running the native backend." >&2
    exit 1
fi

echo "Native runtime dependencies are available."
