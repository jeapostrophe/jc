#!/usr/bin/env bash
# Fetch outline.scm queries from the Zed editor repository.
set -euo pipefail

BASE_URL="https://raw.githubusercontent.com/zed-industries/zed/refs/heads/main/crates/languages/src"
DEST="jc-app/src/outline_queries"

for lang in rust markdown python go javascript typescript; do
  echo "Fetching ${lang}/outline.scm"
  curl -sL "${BASE_URL}/${lang}/outline.scm" -o "${DEST}/${lang}.scm"
done

echo "Done."
