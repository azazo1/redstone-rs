#!/usr/bin/env sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: run.sh SCENARIO OUTPUT_JSONL" >&2
  exit 2
fi

root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
oracle_jar="${VANILLA_ORACLE_JAR:-$root/tools/vanilla-oracle/build/vanilla-oracle.jar}"
client="$root/assets/client-26.1.2.jar"
libraries="$root/assets/libraries/26.1.2"
oracle_libraries="$root/assets/oracle-libraries"

if [ ! -f "$oracle_jar" ]; then
  echo "Java oracle JAR 不存在: $oracle_jar" >&2
  echo "先运行 just oracle-build" >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
prepared="$work/scenario.toml"
if [ -n "${REDSTONE_ORACLE_CONVERTER:-}" ]; then
  "$REDSTONE_ORACLE_CONVERTER" oracle-prepare "$1" "$prepared"
else
  cargo run --quiet -p redstone-cli -- oracle-prepare "$1" "$prepared"
fi

classpath="$oracle_jar:$client:$(find "$libraries" "$oracle_libraries" -name '*.jar' -type f | sort | paste -sd ':' -)"
java -Xmx4g -cp "$classpath" redstone.oracle.Main "$prepared" "$2"
