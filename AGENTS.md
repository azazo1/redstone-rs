# AGENTS

客户端游戏的 client-26.1.2.jar 代码已经反编译在 decompiled/ 当中, 可以当作原版游戏的实现逻辑参考, 如果没有, 运行相关的 just recipe 即可获取 mc 的反编译代码用于学习.

性能优化的时候请逐步编写 [perf.md](docs/perf.md).
samply 使用: `cargo samply run assets/scenarios/cpu-8bit-dvd.toml` 之类的命令, 否则可能生成没有调试符号的结果.
