#!/usr/bin/env sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: run.sh SCENARIO OUTPUT_JSONL" >&2
  exit 2
fi

if [ -z "${VANILLA_ORACLE_JAR:-}" ]; then
  echo "VANILLA_ORACLE_JAR is required" >&2
  exit 2
fi

exec java -Xmx4g -jar "$VANILLA_ORACLE_JAR" "$1" "$2"
