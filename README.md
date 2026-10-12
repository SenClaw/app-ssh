# SSH Manager

Standalone SSH Manager App Space for SenClaw. Provides web terminal and secure command execution.

- MCP: **`ssh-manager-mcp`**

## Build & cài

```bash
scripts/pack.sh            # npm build web + cargo build --release + zip
```

Ra `dist/<platform>/ssh-manager.zip` (và một bản ở gốc repo) — cài qua SenClaw →
Apps → Install, hoặc publish bằng `scripts/publish-app.sh <zip>`.

CI (`.github/workflows/build.yml`) build + test linux-x64, darwin-arm64,
windows-x64 và publish lên hub khi push `main` hoặc tag `v*` (bump `version` trong
`senclaw-hub.json` cho mỗi bản phát hành).

## Chạy local

```bash
cargo run                            # backend
cd web && npm install && npm run dev  # UI dev
cargo test
```
