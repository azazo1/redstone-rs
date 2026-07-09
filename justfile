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
