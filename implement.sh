#!/usr/bin/env bash
set -euo pipefail
MAX_RUNS=4
cd "$(dirname "$0")"

result=$(mktemp)
trap 'rm -f "$result"' EXIT

for ((run = 1; run <= MAX_RUNS; run++)); do
  echo "Implementation pass $run/$MAX_RUNS"
  : > "$result"
  {
    cat prompt.md
    printf '\nPass %s/%s. Review existing work and continue; verify completion again.\n' "$run" "$MAX_RUNS"
  } | codex exec --approve-for-me -m "${CODEX_MODEL:-gpt-6.1-sol}" \
    -c 'model_reasoning_effort="xhigh"' -o "$result" -

  if [[ $(tail -n 1 "$result") == CODEX_DONE ]]; then
    echo "Complete after $run passes."
    exit 0
  fi
done

echo "Stopped after $MAX_RUNS passes without confirmed completion." >&2
exit 1
