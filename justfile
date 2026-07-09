VINEFLOWER := "tools/vineflower-1.12.0.jar"
CLIENT_JAR := "assets/client-26.1.2.jar"
OUT_DIR := "decompiled/client-26.1.2"

[private]
default:
    @just --list

# just decompile
# 使用 Vineflower 反编译客户端 JAR.
decompile: prepare-tools
    java -Xmx4g -jar {{ VINEFLOWER }} --folder --skip-extra-files=true --thread-count=8 {{ CLIENT_JAR }} {{ OUT_DIR }}

# just prepare-tools
# 检查工具是否存在, 如果不存在则下载.
prepare-tools:
    #!/usr/bin/env sh
    set -eu
    mkdir -p tools
    if [ -f "{{ VINEFLOWER }}" ]; then
      echo "Vineflower 已存在: {{ VINEFLOWER }}"
    else
      echo "下载 Vineflower 1.12.0"
      curl -fL https://repo1.maven.org/maven2/org/vineflower/vineflower/1.12.0/vineflower-1.12.0.jar -o "{{ VINEFLOWER }}"
    fi

# just download-client 26.1.2
# 下载指定版本的 Minecraft client JAR.
download-client version="26.1.2":
    #!/usr/bin/env sh
    set -eu
    mkdir -p assets
    manifest="$(mktemp)"
    version_json="$(mktemp)"
    trap 'rm -f "$manifest" "$version_json"' EXIT
    echo "读取 Mojang 版本清单"
    curl -fsSL https://piston-meta.mojang.com/mc/game/version_manifest_v2.json -o "$manifest"
    version_url="$(jq -r --arg version "{{ version }}" '.versions[] | select(.id == $version) | .url // empty' "$manifest")"
    if [ -z "$version_url" ]; then
      echo "未找到版本: {{ version }}" >&2
      exit 1
    fi
    echo "读取版本信息: {{ version }}"
    curl -fsSL "$version_url" -o "$version_json"
    client_url="$(jq -r '.downloads.client.url // empty' "$version_json")"
    if [ -z "$client_url" ]; then
      echo "版本缺少 client 下载地址: {{ version }}" >&2
      exit 1
    fi
    echo "下载 client JAR: assets/client-{{ version }}.jar"
    curl -fL "$client_url" -o "assets/client-{{ version }}.jar"

# just download-server 26.1.2
# 下载并校验指定版本的 Minecraft dedicated server JAR.
download-server version="26.1.2":
    #!/usr/bin/env sh
    set -eu
    mkdir -p assets
    manifest="$(mktemp)"
    version_json="$(mktemp)"
    trap 'rm -f "$manifest" "$version_json"' EXIT
    echo "读取 Mojang 版本清单"
    curl -fsSL https://piston-meta.mojang.com/mc/game/version_manifest_v2.json -o "$manifest"
    version_url="$(jq -r --arg version "{{ version }}" '.versions[] | select(.id == $version) | .url // empty' "$manifest")"
    if [ -z "$version_url" ]; then
      echo "未找到版本: {{ version }}" >&2
      exit 1
    fi
    echo "读取版本信息: {{ version }}"
    curl -fsSL "$version_url" -o "$version_json"
    server_url="$(jq -r '.downloads.server.url // empty' "$version_json")"
    server_sha1="$(jq -r '.downloads.server.sha1 // empty' "$version_json")"
    if [ -z "$server_url" ] || [ -z "$server_sha1" ]; then
      echo "版本缺少 server 下载信息: {{ version }}" >&2
      exit 1
    fi
    output="assets/server-{{ version }}.jar"
    if [ -f "$output" ]; then
      actual_sha1="$(shasum -a 1 "$output" | awk '{print $1}')"
      if [ "$actual_sha1" = "$server_sha1" ]; then
        echo "server JAR 已存在且校验通过"
        exit 0
      fi
      echo "现有 server JAR 校验失败, 重新下载"
    fi
    echo "下载 server JAR: $output"
    curl -fL "$server_url" -o "$output"
    actual_sha1="$(shasum -a 1 "$output" | awk '{print $1}')"
    if [ "$actual_sha1" != "$server_sha1" ]; then
      echo "server JAR SHA-1 校验失败" >&2
      rm -f "$output"
      exit 1
    fi
    echo "server JAR 校验完成"

# just gametest-oracle 26.1.2 --help
# 使用项目内运行目录启动官方 GameTest server.
gametest-oracle version="26.1.2" *args: (download-server version)
    #!/usr/bin/env sh
    set -eu
    runtime_dir=".gametest/{{ version }}"
    mkdir -p "$runtime_dir"
    echo "启动 GameTest oracle: $runtime_dir"
    exec java \
      -DbundlerMainClass=net.minecraft.gametest.Main \
      -DbundlerRepoDir="$runtime_dir" \
      -jar "assets/server-{{ version }}.jar" \
      --universe "$runtime_dir/world" \
      {{args}}

# 生成最小的 block-based GameTest oracle datapack.
oracle-init output=".gametest/oracle-pack":
    cargo run -- oracle-init --output {{ output }}

# just oracle-structure machine.nbt .gametest/oracle-pack/data/redstone_oracle/structure/machine.nbt
# 导出结构为 Java GameTest 可读取的 gzip NBT 模板.
oracle-structure structure output:
    cargo run -- oracle-structure --structure {{ structure }} --output {{ output }}

# 运行 Rust 静态检查.
clippy:
    cargo clippy --tests

# 运行仿真器测试.
test:
    cargo test

# just run benchmark --blocks 100000 --ticks 1000
# 运行命令行仿真器.
run *args:
    cargo run -- {{args}}
