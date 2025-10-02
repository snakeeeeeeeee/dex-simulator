#!/usr/bin/env bash
set -euo pipefail

# 简易环境初始化脚本，安装所需工具并校验 Rust 版本。

if ! command -v rustup >/dev/null 2>&1; then
  echo "[错误] 未检测到 rustup，请先安装 Rust 工具链" >&2
  exit 1
fi

RUST_VERSION="stable"
echo "使用 rustup 切换到 ${RUST_VERSION} 工具链"
rustup default "${RUST_VERSION}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "[错误] 未检测到 cargo" >&2
  exit 1
fi

echo "环境检查通过，可执行 cargo build 验证依赖"
