#!/usr/bin/env bash
#
# Refuse two tracked files whose names differ only in case.
#
# Linux is case-sensitive and Windows and macOS are not, so a repository that
# builds here can be unbuildable there — and this is not a theoretical risk,
# it happened. `ui/src/pages/automation/` held both `Compare.tsx` (the
# component) and `compare.ts` (its React-free reasoning). On Linux an
# `import … from './Compare'` cannot match `compare.ts`, so it resolves to
# `Compare.tsx` and all is well. On Windows it matches, because TypeScript
# tries `Compare.ts` before `Compare.tsx` and the filesystem says that file
# exists — so the import silently loads the wrong module and the build fails
# on an export that is not there. Three screens had five such pairs, the
# release build died on the first of them, and no Windows installer had ever
# been produced because of it.
#
# Two checks, because they catch different mistakes:
#
#   1. Two tracked paths that are the same when lowercased. Git can hold both;
#      a case-insensitive checkout cannot, and one silently overwrites the
#      other.
#   2. Within one directory, two files whose names match when lowercased once
#      the extension is removed — the `Compare.tsx` / `compare.ts` shape. Only
#      a problem for languages that resolve a module without its extension,
#      which is why it is limited to the interface's sources. JavaScript
#      counts as well as TypeScript: `printFrame.js` beside `PrintFrame.tsx`
#      got past a check that looked at `.ts` and `.tsx` alone, and the print
#      frame's tests then failed on Windows only.
#
# Files that are not committed yet are looked at too (ignored ones are not),
# so the answer is the same before a commit as after it.
set -euo pipefail

cd "$(dirname "$0")/.."
status=0

same_path=$(git ls-files --cached --others --exclude-standard | awk '{ key = tolower($0); if (key in seen) print seen[key] "\n" $0; seen[key] = $0 }')
if [ -n "$same_path" ]; then
    echo "Two tracked files differ only in case, so a case-insensitive checkout cannot hold both:"
    echo "$same_path" | sed 's/^/  /'
    status=1
fi

same_stem=$(git ls-files --cached --others --exclude-standard 'ui/src/**' | awk -F/ '
    /\.(ts|tsx|js|jsx|mjs|cjs)$/ {
        dir = ""
        for (i = 1; i < NF; i++) dir = dir $i "/"
        stem = $NF
        sub(/\.(ts|tsx|js|jsx|mjs|cjs)$/, "", stem)
        key = dir tolower(stem)
        if (key in seen && seen[key] != stem) print dir ": " seen[key] " and " stem
        seen[key] = stem
    }')
if [ -n "$same_stem" ]; then
    echo "Two modules in one directory differ only in case, so an import of either"
    echo "resolves to the wrong one on Windows and macOS:"
    echo "$same_stem" | sed 's/^/  /'
    echo
    echo "Rename one of them. The convention here is that a component keeps its"
    echo "own name (it is its export) and the React-free module beside it is"
    echo "named for what it does, not for the component — 'comparison.ts' next"
    echo "to 'Compare.tsx', not 'compare.ts'."
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "No two files differ only in case."
fi
exit "$status"
