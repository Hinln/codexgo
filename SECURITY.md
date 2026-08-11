# Security Policy

## 安全边界

CodexGo 只负责本机 Provider 切换，不代理模型流量，也不管理
OpenAI/ChatGPT 登录。

- `auth.json`：只检查文件是否存在，不读取、复制、删除或修改其内容。
- API Key：验证成功后只写入当前 Windows 用户的 `VEXLUNE_HUB_API_KEY` 环境变量。
- 配置：使用 TOML 结构化编辑和原子写入，不把 Key 写入 `config.toml`。
- 会话：只处理已确认的 `model_provider` 元数据，不记录消息正文。
- 网络：远程 API 必须使用 HTTPS；仅回环地址允许 HTTP。
- 重定向：阻止跨协议、跨域名和跨端口重定向。
- 日志：不记录 Key、Authorization、Token、提示词、会话正文或用户源码。
- 更新：没有自动更新、远程程序下载或执行功能。
- 推广链接：只允许系统浏览器打开 `https://hub.vexlune.com`。

## 报告安全问题

请通过仓库的安全报告渠道提交最小复现、影响范围和版本号。请勿附带真实 API Key、
OpenAI Token、`auth.json` 内容、会话正文或用户项目内容。

## Key 泄露处置

如果 API Key 曾出现在聊天、截图、终端历史或其他非预期位置，请立即在对应服务商后台
轮换，并从当前用户环境变量中删除旧值。
