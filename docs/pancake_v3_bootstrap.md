# PancakeSwap V3 预加载说明

可在 `config/pancake_v3_bootstrap.yaml` 中维护需要追踪的 V3 池子，字段示例：

```yaml
- pool: "0x..."
  token0:
    address: "0x..."
    symbol: "WBNB"
    decimals: 18
  token1:
    address: "0x..."
    symbol: "USDT"
    decimals: 18
```

运行时调用 `PancakeV3EventHandler::from_config_file` 生成 handler，即可让 router 根据池子地址路由事件。
*** End Patch
PATCH
