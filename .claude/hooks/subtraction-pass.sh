#!/usr/bin/env bash
# Stop hook — nudge one subtraction pass before finishing when source changed.
# Canon stays small by default; this makes "what can I remove?" automatic
# instead of something the user has to ask for every time.
#
# Fires at most once per stop sequence: the stop_hook_active guard means that
# after Claude does the pass and stops again, this exits quietly. No loop.

input=$(cat)

case "$input" in
  *'"stop_hook_active":true'* | *'"stop_hook_active": true'*) exit 0 ;;
esac

# Only nudge when tracked source/docs actually changed (staged or unstaged).
changed=0
git diff --quiet -- '*.rs' '*.can' '*.md' 2>/dev/null || changed=1
git diff --cached --quiet -- '*.rs' '*.can' '*.md' 2>/dev/null || changed=1
[ "$changed" -eq 0 ] && exit 0

cat <<'EOF'
{"decision":"block","reason":"Subtraction pass before finishing. Re-read your diff and remove anything not pulling its weight: dead code, legacy/compat shims, speculative options, unrequested docs or features. Aim for a net-negative diff. Then state plainly what you removed — or that nothing needed removing. Do not add anything new in this pass."}
EOF
exit 0
