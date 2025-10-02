# 本地 DEX 模拟器

本项目旨在构建一个可扩展的本地 DEX 模拟器，通过监听链上事件并在本地复现池子状态，帮助套利策略快速评估多路径收益。

## 项目结构

```text
.
├── Cargo.toml              # 工作区配置
├── README.md               # 项目说明
├── config/                 # 默认配置与环境参数
├── docs/                   # 需求、计划与设计文档
├── crates/
│   └── core/               # 核心库（事件、状态、模拟、路径等模块）
└── scripts/                # 常用脚本
```

核心库已预置以下模块骨架：

- `config`：加载与管理全局配置，支持 YAML/JSON/TOML。
- `logging`：基于 log4rs 的日志初始化，默认输出到控制台。
- `event`：事件监听与回放接口定义。
- `state`：池子状态抽象与快照结构。
- `simulation`：Swap 模拟请求/结果与计算器接口。
- `syncer`：状态同步与校验接口。
- `path`：路径搜索约束与接口。
- `types`：通用类型定义（资产、池子、路径等）。

## 快速开始

1. **初始化环境**

```bash
./scripts/setup_env.sh
```

2. **加载依赖并验证编译**

```bash
cargo check
```

3. **运行示例**

默认配置位于 `config/default.yaml`，可按需替换 RPC 节点。详细运行说明与常用命令请参见 `docs/运行指南.md`。

## 下一步计划

开发计划详见 `docs/开发计划.md`，当前阶段聚焦核心架构搭建与 PancakeSwap V2 支持。
