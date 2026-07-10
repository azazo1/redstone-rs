#!/usr/bin/env sh
set -eu

root="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)"
client="$root/assets/client-26.1.2.jar"
libraries="$root/assets/libraries/26.1.2"
oracle_libraries="$root/assets/oracle-libraries"
source="$root/tools/vanilla-oracle/src"
build="$root/tools/vanilla-oracle/build"
classes="$build/classes"
output="$build/vanilla-oracle.jar"

if [ ! -f "$client" ]; then
  echo "缺少 Minecraft client JAR: $client" >&2
  exit 2
fi
if [ ! -d "$libraries" ]; then
  echo "缺少 Minecraft 运行库: $libraries" >&2
  echo "先运行 just download-runtime 26.1.2" >&2
  exit 2
fi
if [ ! -d "$oracle_libraries" ]; then
  echo "缺少 Java oracle 运行库: $oracle_libraries" >&2
  echo "先运行 just download-oracle-deps" >&2
  exit 2
fi

mkdir -p "$classes"
classpath="$client:$(find "$libraries" "$oracle_libraries" -name '*.jar' -type f | sort | paste -sd ':' -)"
sources="$build/sources.txt"
find "$source" -name '*.java' -type f | sort > "$sources"
total="$(wc -l < "$sources" | tr -d ' ')"
if [ "$total" -eq 0 ]; then
  echo "没有找到 Java oracle 源码" >&2
  exit 2
fi

echo "编译 Java oracle: $total 个源文件"
javac --release 25 -encoding UTF-8 -cp "$classpath" -d "$classes" @"$sources"
jar --create --file "$output" --main-class redstone.oracle.Main -C "$classes" .
echo "Java oracle 已构建: $output"
