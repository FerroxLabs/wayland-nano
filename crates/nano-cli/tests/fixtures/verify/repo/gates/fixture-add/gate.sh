#!/usr/bin/env bash
set -eu
artifact="$1"
artifact="${artifact#\\\\?\\}"
artifact="${artifact//\\//}"
content="$(<"$artifact")"
if [[ "$content" == *"a + b"* || "$content" == "diff --git "* ]]; then
  echo 'fixture-add-gate: 1/1'
  exit 0
fi
echo 'FAIL FX-01 value'
echo 'fixture-add-gate: 0/1'
exit 7
