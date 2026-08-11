CodexGo 1.0.0 — by Vexlune Hub

1. Enter an OpenAI-compatible API root address and API Key.
2. Do not include /v1. If supplied, trailing /v1 segments are removed automatically.
3. The app appends exactly one /v1 for Models/Responses requests and Codex configuration.
4. Available models load automatically after both address and key are entered; the refresh button remains available for retry.
5. Switching changes model-request routing only. ChatGPT sign-in and existing Codex conversations remain available.
6. Remote APIs require HTTPS. Localhost and loopback endpoints may use HTTP.
7. The Vexlune Hub button opens https://hub.vexlune.com in the system browser.

The managed Provider uses requires_openai_auth = true and stores the request credential in VEXLUNE_HUB_API_KEY.
The app never reads, copies, deletes, or modifies auth.json contents.
