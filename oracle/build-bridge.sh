#!/usr/bin/env sh
set -eu

version="${1:-26.1.2}"
server="assets/server-${version}.jar"
build_dir="oracle/build/${version}"
lock_dir="${build_dir}.lock"
unpacked="${build_dir}/unpacked"
classes="${build_dir}/classes"
bridge_jar="${build_dir}/oracle-bridge-${version}.jar"
bundled_server="${build_dir}/redstone-oracle-server-${version}.jar"
library_path="io/redstoners/oracle-bridge/${version}/oracle-bridge-${version}.jar"

if [ ! -f "$server" ]; then
  echo "缺少官方 server JAR: $server" >&2
  exit 1
fi

mkdir -p "oracle/build"
while ! mkdir "$lock_dir" 2>/dev/null; do
  echo "等待另一个 GameTest bridge 构建完成"
  sleep 1
done
trap 'rmdir "$lock_dir"' EXIT

echo "准备 GameTest bridge: ${version}"
rm -rf "$build_dir"
mkdir -p "$unpacked" "$classes"
unzip -q "$server" -d "$unpacked"

classpath="${unpacked}/META-INF/versions/${version}/server-${version}.jar"
for library in $(find "${unpacked}/META-INF/libraries" -name '*.jar' -type f | sort); do
  classpath="${classpath}:$library"
done

echo "编译 GameTest bridge"
javac -cp "$classpath" -d "$classes" oracle/bridge/src/main/java/io/redstoners/oracle/OracleMain.java
jar --create --date=2020-01-01T00:00:00Z --file "$bridge_jar" -C "$classes" .

echo "封装带 bridge 的 GameTest server"
mkdir -p "${build_dir}/patch/META-INF/libraries/$(dirname "$library_path")"
cp "$bridge_jar" "${build_dir}/patch/META-INF/libraries/${library_path}"
sha256="$(shasum -a 256 "$bridge_jar" | awk '{print $1}')"
unzip -p "$server" META-INF/libraries.list > "${build_dir}/patch/META-INF/libraries.list"
printf '%s\t%s\t%s\n' "$sha256" "io.redstoners:oracle-bridge:${version}" "$library_path" >> "${build_dir}/patch/META-INF/libraries.list"
cp "$server" "$bundled_server"
(cd "${build_dir}/patch" && zip -q -r "../redstone-oracle-server-${version}.jar" META-INF)

echo "bridge server ready: $bundled_server"
