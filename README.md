<p align="center">
  <img src="public/vexlune-vh-mark.png" width="112" alt="Vexlune Hub logo">
</p>

<h1 align="center">CodexGo</h1>

<p align="center">
  <strong>CodexGo is a Windows desktop tool that safely switches Codex Desktop API providers while keeping your ChatGPT login state and existing conversations.</strong>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://github.com/Hinln/codexgo/releases/latest/download/CodexGo-v1.0.0-windows-x64.exe"><strong>Download for Windows x64</strong></a>
  · <a href="https://github.com/Hinln/codexgo/releases/latest">Release notes</a>
  · <a href="https://github.com/Hinln/codexgo/releases/latest/download/CodexGo-v1.0.0-windows-x64-SHA256.txt">SHA-256</a>
</p>

> Version 1.0.0 · Windows 10/11 x64 · MIT License<br>
> Developed by Hinln under the Vexlune Hub name.

![CodexGo demo](docs/demo.gif)

## Why CodexGo

Codex Desktop account identity and model-request routing are separate concerns. You may want to keep your ChatGPT sign-in, subscription visibility, existing tasks, and conversations while temporarily sending model requests to a self-hosted gateway, a local service, or another OpenAI-compatible API.

Manually editing `config.toml`, environment variables, and session-provider metadata is fragile. It can produce `/v1/v1` URLs, expose API keys, leave a half-applied configuration, or make rollback difficult. CodexGo turns those changes into a visible, verifiable, and recoverable desktop transaction.

## Features

- Keeps your ChatGPT login state.
- Keeps existing Codex conversations and task content.
- Switches between the original route and OpenAI-compatible API providers.
- Discovers available models automatically after the API URL and key are entered.
- Normalizes API URLs so the effective base URL contains exactly one trailing `/v1`.
- Creates a safe configuration backup and can roll back a failed switch.
- Applies provider changes as a guarded transaction with step-by-step status.
- Uses no local proxy and does not change DNS, `hosts`, system proxy settings, or certificates.
- Keeps API keys out of `config.toml`, application data, and diagnostic logs.

## Support

If CodexGo helps your workflow,
please consider giving this project a ⭐ on GitHub.

Your star helps more developers discover CodexGo.

## How it differs

The comparison below describes common implementation patterns, not any particular project.

| Area | Common approach | CodexGo |
|---|---|---|
| Request routing | Runs a resident local proxy | Writes the Codex provider configuration directly; no proxy |
| ChatGPT identity | Mixes account sign-in with a third-party key | Keeps identity and model-request credentials separate with `requires_openai_auth = true` |
| API URL | Leaves `/v1` handling to the user | Normalizes input, backend requests, and final configuration to exactly one `/v1` |
| Models | Requires manual model IDs | Calls the Models API automatically and also offers manual refresh |
| Configuration | Replaces text or overwrites the whole file | Uses structured TOML editing, preserves unrelated values and comments, then writes atomically |
| Failure handling | Requires manual repair | Backs up configuration, sessions, and environment state; rolls back failed commits |
| Conversation continuity | Ignores sessions or rewrites whole files | Changes only confirmed `model_provider` metadata, not messages, task IDs, working directories, or tool calls |
| API keys | Stores keys in configuration or application files | Stores the key only in the current Windows user's `VEXLUNE_HUB_API_KEY` environment variable |
| Restore behavior | Assumes the original route was OpenAI | Restores the real provider, model, and reasoning effort captured before the first switch |

## Screenshots

| Route switcher | API configuration |
|---|---|
| ![CodexGo route switcher](docs/images/dashboard.png) | ![CodexGo API configuration](docs/images/api-config.png) |

### Transaction completed

![CodexGo successful provider switch](docs/images/switch-success.png)

The screenshots use a local demonstration bridge and fictional API credentials. No live API request or real credential was used.

## How it works

```text
Detect Codex → Close Codex → Create backup → Validate API
→ Atomically update Provider → Migrate session metadata → Save environment variable
→ Verify result → Restart Codex
```

If a commit step fails, CodexGo attempts to restore the configuration, session metadata, and environment variable to their pre-operation state. Returning to the original route uses the baseline captured before the first switch rather than guessing what the previous provider was.

### Exactly one `/v1`

The UI stores the API root without a trailing `/v1`; network requests and the Codex provider configuration then add one canonical `/v1`.

| User input | Stored root | Effective API base URL |
|---|---|---|
| `https://api.example.com` | `https://api.example.com` | `https://api.example.com/v1` |
| `https://api.example.com/v1` | `https://api.example.com` | `https://api.example.com/v1` |
| `https://api.example.com/v1/v1/` | `https://api.example.com` | `https://api.example.com/v1` |
| `https://example.com/openai/v1` | `https://example.com/openai` | `https://example.com/openai/v1` |

Remote endpoints must use HTTPS. HTTP is accepted only for loopback hosts such as `localhost` and `127.0.0.1` so local OpenAI-compatible services remain usable.

## Quick start

1. Download `CodexGo-v1.0.0-windows-x64.exe` from the [latest release](https://github.com/Hinln/codexgo/releases/latest).
2. Open **API Configuration**.
3. Enter the API root URL and API key.
4. Wait for available models to load automatically, then select a default model.
5. Choose **Apply and switch**.
6. Review the guarded backup, validation, switching, and restart steps in **Status Monitor**.

To return, select the original route on the Route Switcher page. CodexGo restores the recorded baseline while leaving your ChatGPT account state and conversation content intact.

> The current Windows executable is not Authenticode-signed, so Microsoft Defender SmartScreen may show an unknown-publisher warning. Verify the published SHA-256 before running it.

## Provider configuration

For an input such as `https://api.example.com/v1`, the resulting core configuration is equivalent to:

```toml
model_provider = "vexlune_hub"
model = "your-selected-model"
model_reasoning_effort = "xhigh"

[model_providers.vexlune_hub]
name = "Vexlune Hub Custom API"
base_url = "https://api.example.com/v1"
env_key = "VEXLUNE_HUB_API_KEY"
wire_api = "responses"
requires_openai_auth = true
```

The key is not written to `config.toml`; it is stored in the current user's environment at `HKCU\Environment\VEXLUNE_HUB_API_KEY`.

## Compatibility

The target service should support at least one of:

- `GET /v1/models`;
- an OpenAI Responses API-style `POST /v1/responses` endpoint.

Compatibility varies across providers, model names, parameters, tool calling, and streaming implementations. CodexGo cannot guarantee that every service advertised as “OpenAI-compatible” works fully with Codex. Review the provider's privacy, billing, and data-retention policies before use.

## Security boundaries

- `auth.json`: CodexGo checks only whether it exists; it never reads its contents and never includes it in backups.
- API keys: sensitive strings use `Zeroizing` to reduce their in-memory lifetime and are stored only in the current-user environment variable.
- Configuration: TOML is edited structurally and committed atomically.
- Sessions: JSONL is processed as a stream; SQLite operations use transactions and the backup API.
- Network: cross-scheme, cross-host, and cross-port redirects are blocked.
- Logs: diagnostics are redacted and never include API keys, authorization headers, prompts, or conversation bodies.
- Backups: `%CODEX_HOME%\switcher-backups\`.
- Logs: `%LOCALAPPDATA%\CodexGo\logs\`.

When a custom route is active, model requests and related context are sent to the API service chosen by the user. Selecting and trusting that service remains the user's responsibility.

## Build from source

Requirements: Windows 10/11 x64, Node.js, pnpm, stable Rust, Visual Studio 2022 C++ Build Tools with the Windows SDK, and Microsoft Edge WebView2 Runtime.

```powershell
pnpm install --frozen-lockfile
pnpm build:web
pnpm test:sites
cargo fmt --manifest-path src-tauri\Cargo.toml -- --check
cargo test --manifest-path src-tauri\Cargo.toml
pnpm tauri build --no-bundle
```

The portable executable is generated at `src-tauri\target\release\CodexGo.exe`. The release naming convention is `CodexGo-v{version}-windows-x64.exe`.

## Technology

React 19, TypeScript, Tauri 2, Rust, `reqwest` with rustls, `toml_edit`, `rusqlite`, and Windows Registry/Win32 APIs.

## Vexlune Hub

CodexGo retains the Vexlune Hub attribution. The **Visit Vexlune Hub** button opens [hub.vexlune.com](https://hub.vexlune.com) in the default browser; it never changes the configured API address or request route.

## Contributing

Issues and pull requests are welcome. Changes involving provider configuration, session migration, environment variables, backups, or restore behavior should include tests. Test data must never contain real API keys, tokens, prompts, or user conversation content.

## License

CodexGo is released under the [MIT License](LICENSE).

## Disclaimer

CodexGo is an independent project and is not affiliated with or endorsed by OpenAI. “OpenAI,” “ChatGPT,” and “Codex” belong to their respective owners. Users and their chosen service providers are responsible for third-party API costs, availability, and data handling.

Developer: Hinln
