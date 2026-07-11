MINECRAFT_VERSION := "26.1.2"
MINECRAFT_VERSION_URL := "https://piston-meta.mojang.com/v1/packages/b0004e16ba0789e3e8ef109a2f3808ca200b3c31/26.1.2.json"
VINEFLOWER_VERSION := "1.12.0"
TOMLJ_VERSION := "1.1.1"
ANTLR_VERSION := "4.11.1"
CHECKER_QUAL_VERSION := "3.21.2"
ASM_VERSION := "9.8"

VINEFLOWER := "tools/vineflower-" + VINEFLOWER_VERSION + ".jar"
CLIENT_JAR := "assets/client-" + MINECRAFT_VERSION + ".jar"
OUT_DIR := "decompiled/client-" + MINECRAFT_VERSION
RUNTIME_DIR := "assets/libraries/" + MINECRAFT_VERSION
REPORT_DIR := "crates/redstone-java-26/data/" + MINECRAFT_VERSION
ORACLE_DIR := "tools/vanilla-oracle"
ORACLE_LIB_DIR := "assets/oracle-libraries"

import? 'local.just'

[private]
default:
    @just --list

# 编译整个 workspace.
build:
    cargo build --workspace

# 对整个 workspace 执行 clippy.
clippy:
    cargo clippy --workspace --all-targets

# 运行整个 workspace 的测试.
test:
    cargo test --workspace

# just run path/to/scenario.toml
# 运行一个红石时序场景.
run scenario *args:
    cargo run -p redstone-cli -- run {{ scenario }} {{ args }}

# just inspect path/to/structure.litematic --block 0,0,0 --json
# 检查结构内容和未支持方块.
inspect structure *args:
    cargo run -p redstone-cli -- inspect {{ structure }} {{ args }}

# just bench --ticks 100
# 运行大规模空闲刻基准.
bench *args:
    cargo run --release -p redstone-cli -- bench {{ args }}

# just decompile
# 使用 Vineflower 反编译客户端 JAR.
decompile-client: prepare-tools
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
      echo "下载 Vineflower {{ VINEFLOWER_VERSION }}"
      curl -fL "https://repo1.maven.org/maven2/org/vineflower/vineflower/{{ VINEFLOWER_VERSION }}/vineflower-{{ VINEFLOWER_VERSION }}.jar" -o "{{ VINEFLOWER }}"
    fi

# just download-client 26.1.2
# 下载指定版本的 Minecraft client JAR.
download-client:
    #!/usr/bin/env sh
    set -eu
    mkdir -p assets
    version_json="$(mktemp)"
    trap 'rm -f "$version_json"' EXIT
    echo "读取 Minecraft {{ MINECRAFT_VERSION }} 版本信息"
    curl -fsSL "{{ MINECRAFT_VERSION_URL }}" -o "$version_json"
    client_url="$(jq -r '.downloads.client.url // empty' "$version_json")"
    if [ -z "$client_url" ]; then
      echo "版本缺少 client 下载地址: {{ MINECRAFT_VERSION }}" >&2
      exit 1
    fi
    echo "下载 client JAR: {{ CLIENT_JAR }}"
    curl -fL "$client_url" -o "{{ CLIENT_JAR }}"

# just download-runtime 26.1.2
# 下载指定 Minecraft 版本的数据生成运行库.
download-runtime:
    #!/usr/bin/env sh
    set -eu
    runtime_dir="{{ RUNTIME_DIR }}"
    mkdir -p "$runtime_dir"
    version_json="$(mktemp)"
    libraries="$(mktemp)"
    trap 'rm -f "$version_json" "$libraries"' EXIT
    echo "读取 Minecraft {{ MINECRAFT_VERSION }} 版本信息"
    curl -fsSL "{{ MINECRAFT_VERSION_URL }}" -o "$version_json"
    jq -r '.libraries[].downloads.artifact | select(.url != null) | [.path, .url] | @tsv' "$version_json" > "$libraries"
    total="$(wc -l < "$libraries" | tr -d ' ')"
    current=0
    while IFS="$(printf '\t')" read -r path url; do
      current=$((current + 1))
      target="$runtime_dir/$path"
      if [ ! -f "$target" ]; then
        mkdir -p "$(dirname "$target")"
        curl -fsSL "$url" -o "$target"
      fi
      if [ $((current % 10)) -eq 0 ] || [ "$current" -eq "$total" ]; then
        echo "运行库下载进度: $current/$total"
      fi
    done < "$libraries"

# just generate-reports
# 使用官方数据生成器输出 26.1.2 方块和注册表报告.
generate-reports: download-runtime download-client
    #!/usr/bin/env sh
    set -eu
    mkdir -p "{{ REPORT_DIR }}"
    classpath="{{ CLIENT_JAR }}:$(find "{{ RUNTIME_DIR }}" -name '*.jar' -type f | sort | paste -sd ':' -)"
    echo "生成 Minecraft 26.1.2 数据报告"
    java -Xmx4g -cp "$classpath" net.minecraft.data.Main --reports --output "{{ REPORT_DIR }}"

# 下载 Java oracle 的 TOML 解析运行库.
download-oracle-deps:
    #!/usr/bin/env sh
    set -eu
    mkdir -p "{{ ORACLE_LIB_DIR }}"
    download() {
      name="$1"
      url="$2"
      target="{{ ORACLE_LIB_DIR }}/$name"
      if [ ! -f "$target" ]; then
        echo "下载 Java oracle 运行库: $name"
        curl -fsSL "$url" -o "$target"
      fi
    }
    download "tomlj-{{ TOMLJ_VERSION }}.jar" "https://repo1.maven.org/maven2/org/tomlj/tomlj/{{ TOMLJ_VERSION }}/tomlj-{{ TOMLJ_VERSION }}.jar"
    download "antlr4-runtime-{{ ANTLR_VERSION }}.jar" "https://repo1.maven.org/maven2/org/antlr/antlr4-runtime/{{ ANTLR_VERSION }}/antlr4-runtime-{{ ANTLR_VERSION }}.jar"
    download "checker-qual-{{ CHECKER_QUAL_VERSION }}.jar" "https://repo1.maven.org/maven2/org/checkerframework/checker-qual/{{ CHECKER_QUAL_VERSION }}/checker-qual-{{ CHECKER_QUAL_VERSION }}.jar"
    download "asm-{{ ASM_VERSION }}.jar" "https://repo1.maven.org/maven2/org/ow2/asm/asm/{{ ASM_VERSION }}/asm-{{ ASM_VERSION }}.jar"

# 构建 Java 26.1.2 oracle JAR.
oracle-build: download-runtime download-oracle-deps
    sh {{ ORACLE_DIR }}/build.sh

# 校验 Java 版本, Bootstrap 和注册表初始化.
oracle-self-test: oracle-build
    #!/usr/bin/env sh
    set -eu
    classpath="{{ ORACLE_DIR }}/build/vanilla-oracle.jar:{{ CLIENT_JAR }}:$(find "{{ RUNTIME_DIR }}" "{{ ORACLE_LIB_DIR }}" -name '*.jar' -type f | sort | paste -sd ':' -)"
    java -Xmx2g -cp "$classpath" redstone.oracle.Main --self-test

# 启动内置 always_pass GameTest 并等待原版服务端自动退出.
oracle-server-self-test: oracle-build
    #!/usr/bin/env sh
    set -eu
    classpath="{{ ORACLE_DIR }}/build/vanilla-oracle.jar:{{ CLIENT_JAR }}:$(find "{{ RUNTIME_DIR }}" "{{ ORACLE_LIB_DIR }}" -name '*.jar' -type f | sort | paste -sd ':' -)"
    java -Xmx2g -cp "$classpath" redstone.oracle.Main --server-self-test

# 执行真实 structure, 动作和探针场景并校验输出.
oracle-scenario-self-test: oracle-build
    #!/usr/bin/env sh
    set -eu
    classpath="{{ ORACLE_DIR }}/build/vanilla-oracle.jar:{{ CLIENT_JAR }}:$(find "{{ RUNTIME_DIR }}" "{{ ORACLE_LIB_DIR }}" -name '*.jar' -type f | sort | paste -sd ':' -)"
    java -Xmx4g -cp "$classpath" redstone.oracle.Main --scenario-self-test

# just oracle assets/scenarios/seg7.toml /tmp/seg7-oracle.jsonl
# 使用 Java GameTest oracle 运行场景并输出 JSONL.
oracle scenario output="/tmp/redstone-oracle.jsonl": oracle-build
    sh {{ ORACLE_DIR }}/run.sh "{{ scenario }}" "{{ output }}"

# 使用 Rust 仿真器执行全部原理图场景并与 Java oracle 对比.
test-schematic-scenarios parallel="1": oracle-build
    #!/usr/bin/env sh
    set -eu
    if [ {{ parallel }} -le 1 ]; then
      for scenario in assets/scenarios/*.toml; do
        echo "测试原理图场景: $scenario"
        cargo run -p redstone-cli -- test "$scenario" --oracle
      done
    else
      cargo run -p redstone-cli -- test "assets/scenarios" --oracle # todo 限制并发上限
    fi
