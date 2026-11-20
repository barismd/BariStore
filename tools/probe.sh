#!/bin/sh
# BariStore device probe launcher (MIT).
#
# One command in a-Shell, from anywhere:
#   curl -fsSL https://raw.githubusercontent.com/barismd/BariStore/claude/awesome-maxwell-0fhm39/tools/probe.sh | sh
#
# It fetches the newest tools/probe.py of this branch (pinned to the newest
# commit, so a stale GitHub cache cannot mix two versions), runs it, and leaves
# the report in ~/Documents/baristore_probe/report.txt (also copied to the
# clipboard when possible). Optional: `| sh -s -- env net` runs only those parts.

REPO="barismd/BariStore"
BRANCH="claude/awesome-maxwell-0fhm39"
DIR="$HOME/Documents/baristore_probe"

mkdir -p "$DIR" || { echo "cannot create $DIR"; exit 1; }

SHA=$(curl -sSL --max-time 25 "https://github.com/$REPO/commits/$BRANCH.atom" 2>/dev/null | python3 -c "
import re, sys
m = re.search(r'/commit/([0-9a-f]{40})', sys.stdin.read())
print(m.group(1) if m else '')
" 2>/dev/null)
REF="${SHA:-$BRANCH}"

if ! curl -fsSL --max-time 60 -H 'Cache-Control: no-cache' \
        "https://raw.githubusercontent.com/$REPO/$REF/tools/probe.py" -o "$DIR/probe.py"; then
    echo "download failed (ref $REF)"
    exit 1
fi

python3 "$DIR/probe.py" --ref "$REF" "$@"
