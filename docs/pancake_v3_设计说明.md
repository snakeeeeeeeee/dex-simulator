# PancakeSwap V3 设计说明

本文档概述 Dex Simulator 在 PancakeSwap V3 模块中的设计与实现，涵盖事件解析、状态建模、模拟流程与配置约定。

## 1. 模块结构

核心代码位于 `crates/core/src/dex/pancake_v3/`，主要包含以下组件：

| 模块 | 说明 |
| --- | --- |
| `event.rs` | 定义并解析 V3 事件（Initialize/Mint/Burn/Swap/Collect），输出统一的 `PancakeV3Event` 枚举。 |
| `state.rs` | 描述 `PancakeV3PoolState`，封装 slot0、tick、liquidity 与 Tick bitmap 的增量更新逻辑。 |
| `handler.rs` | 实现 `PancakeV3EventHandler`，负责路由事件、初始化池子快照并落库。 |
| `calculator.rs` | 提供 `PancakeV3SwapCalculator`，基于当前 sqrtPrice 与 fee 估算单跳 swap。 |

CLI 侧在 `crates/app/src/commands/` 中通过监听与模拟命令调度该模块。

## 2. 事件解析

- 所有 Topic 使用 `keccak256(signature)` 生成，并在 `event.rs` 中以 `Lazy` 常量缓存。
- `TryFrom<&EventEnvelope>` 将链上日志转换为强类型事件：
  - `Initialize`：同步 `sqrt_price_x96` 与当前 `tick`。
  - `Mint`/`Burn`：更新 Tick liquidity 和储备。
  - `Swap`：处理正负 amount，调整储备、价格、tick。
  - `Collect`：记录费用领取（当前状态不持久化，用于未来扩展）。

解析失败返回 `PancakeV3EventError::Decode`，由上层处理重试或忽略。

## 3. 状态建模

`PancakeV3PoolState` 继承 `PoolState` trait，关键字段：

- `sqrt_price_x96`：最新价格（Q64.96 格式）。
- `liquidity`：当前可用集中流动性。
- `tick`：当前 tick 指针，使用 `i32`。
- `ticks: HashMap<i32, TickInfo>`：记录 `liquidity_net` / `liquidity_gross`，支持 Mint/Burn 时增量更新。
- `reserves: Reserves`：便于统一快照格式，与 V2 接口保持一致。

状态更新流程：

1. CLI 或监听器读取快照并还原 `PancakeV3PoolState`。
2. 每当事件到达，调用 `update_from_event`：
   - 对于 `Mint`，在 `tick_lower`/`tick_upper` 更新 liquidity net，并累加储备。
   - 对于 `Burn`，执行逆操作。
   - 对于 `Swap`，处理带符号的 amount0/amount1，确保储备不为负。
3. 更新完成后使用 `state.snapshot()` 生成新的 `PoolSnapshot`，写入仓库与快照存储。

## 4. 事件处理器

`PancakeV3EventHandler` 在构造时需要提供：

- 预配置的池子列表（`config/pancake_v3_bootstrap.yaml`），确保初始 token 元数据与地址映射。
- `PoolRepository` 与 `SnapshotStore`，分别用于内存态和持久化。

事件处理流程：

1. `handler.matches()` 通过池子地址判断是否关注。
2. `handle_parsed_event` 拉取或初始化池子状态，应用事件更新后持久化。
3. 在监听模式下，CLI 会把处理器注册到 `DexEventRouter`，由通用事件源驱动。

## 5. 模拟与 CLI

- `simulate_swap_v3` 命令调用 `build_v3_components`：
  - 构建演示用池子、触发 `Initialize`/`Mint` 事件，生成初始快照。
  - 通过 `PancakeV3SwapCalculator` 根据输入路径与 fee 估算输出。
- `PancakeV3SwapCalculator` 当前实现单跳报价，计算逻辑：
  - 读取快照中的价格（`sqrt_price_x96`）与手续费。
  - 基于 `amount_in` 扣除手续费后换算成对侧金额。
  - 不涉及复杂 Tick 穿越，后续可扩展为实际的段落积分计算。

## 6. 配置与扩展

- `config/default.yaml` 可通过 `dex.pancake_v3_bootstrap` 指定追踪池子列表；格式见 `docs/pancake_v3_bootstrap.md`。
- 监听命令在真实模式下，会自动根据快照仓库与 bootstrap 汇总订阅地址，避免遗漏。
- 当前已支持事件回放与监控日志，后续扩展方向：
  - 引入多 fee tier 的路径搜索。
  - 在模拟计算中处理 Tick 穿越及流动性分段。
  - 输出 Collect 事件统计、手续费收入等指标。

## 7. 注意事项

- 公共 RPC 节点存在限流，建议使用稳定 WS/HTTP，并适度降低 `event_backfill_blocks` 与 `backfill_chunk_size`。
- V3 状态恢复依赖 bootstrap 配置，如果新增池子，需要在监听启动前配置好元数据。
- 模拟模块暂未处理多跳路径和滑点控制，需要策略层自行评估风险。

如需深入接口定义或未来计划，请参阅 `docs/开发计划.md` 中的阶段性任务。
