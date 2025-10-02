#!/usr/bin/env bash
set -euo pipefail

# 该脚本用于在本地 fork BSC 调试 PancakeSwap V2 事件同步与模拟。
# 先决条件：安装 foundry (anvil)、jq、以及可用的 BSC RPC URL。

if ! command -v anvil >/dev/null 2>&1; then
  echo "[错误] 未检测到 anvil，请安装 foundry (https://book.getfoundry.sh/)" >&2
  exit 1
fi

export BSC_RPC_URL="http://rpcx.1111.lat/bsc"

if [[ -z "${BSC_RPC_URL:-}" ]]; then
  echo "[错误] 请通过环境变量 BSC_RPC_URL 提供可用的 BSC RPC 地址" >&2
  exit 1
fi

FORK_BLOCK=${FORK_BLOCK:-"latest"}
ANVIL_PORT=${ANVIL_PORT:-8545}
ANVIL_LOG="anvil.log"

echo "[信息] 启动 anvil fork (RPC: ${BSC_RPC_URL}, block: ${FORK_BLOCK})"
nohup anvil --fork-url "${BSC_RPC_URL}" --port "${ANVIL_PORT}" >"${ANVIL_LOG}" 2>&1 &
ANVIL_PID=$!
trap 'kill ${ANVIL_PID} >/dev/null 2>&1 || true' EXIT

sleep 3
if ! kill -0 "${ANVIL_PID}" >/dev/null 2>&1; then
  echo "[错误] anvil 未正常启动，请检查 ${ANVIL_LOG}" >&2
  exit 1
fi

export DEX_SIM_ANVIL_RPC="http://127.0.0.1:${ANVIL_PORT}"
export DEX_SIM_WS_RPC="ws://127.0.0.1:${ANVIL_PORT}"

cat <<JSON
{
  "network": {
    "http_endpoint": "${DEX_SIM_ANVIL_RPC}",
    "ws_endpoint": "${DEX_SIM_WS_RPC}",
    "fork_block": "${FORK_BLOCK}"
  },
  "steps": [
    "# 在一个终端运行：target/debug/dex-simulator-cli listen --sample-events 0",
    "# 在另一个终端使用 bench-swap / simulate-swap 验证输出",
    "# 结合 anvil.log 观察事件推送情况"
  ]
}
JSON

wait ${ANVIL_PID}
