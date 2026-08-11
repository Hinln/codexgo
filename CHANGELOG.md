# Changelog

## 1.0.0 - 2026-08-11

- 发布 CodexGo，由 Vexlune Hub 开发，不修改或替代 Vexlune 专用版。
- 支持用户填写 OpenAI 兼容 API 根地址和 API Key。
- 自动移除末尾一个或多个 `/v1`，请求和 Codex 配置始终只添加一个 `/v1`。
- 地址与密钥填写完成后自动获取可用模型，并保留手动刷新功能。
- 远程地址强制 HTTPS，回环地址允许 HTTP。
- 使用独立 Provider ID `vexlune_hub` 和环境变量 `VEXLUNE_HUB_API_KEY`。
- 保持 `requires_openai_auth = true`，保留 ChatGPT 登录态与已有会话。
- 保留事务备份、失败回滚、原 Provider 恢复、会话元数据迁移和脱敏日志。
- 增加侧栏与关于页的 Vexlune Hub 入口，仅打开 `https://hub.vexlune.com`。
