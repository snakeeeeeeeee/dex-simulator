#!/usr/bin/env bash
set -euo pipefail

# 该脚本演示如何生成 "成功衡量指标" 所需的简单报告。
# 依赖：jq, cargo, bench-swap 输出。

OUTPUT_JSON="benchmark_report.json"
ITERATIONS=${ITERATIONS:-50}
POOL=${POOL:-sample}
AMOUNT=${AMOUNT:-1000000000000000000}
CONFIG=${CONFIG:-config/forknet.yaml}

cargo run --release -- --config "$CONFIG" bench-swap --pool "$POOL" --amount "$AMOUNT" --iterations "$ITERATIONS" > "$OUTPUT_JSON"

echo "=== 结果摘要 ==="
cat "$OUTPUT_JSON"

echo "\n提示: 请结合输出中的 avg/min/max 与 slippage 喜好判断是否达成设定的成功指标。"
