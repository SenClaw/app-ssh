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

## Đồng bộ với một SenClaw khác

Hosts, keychain, port forwarding và snippets có thể được giữ giống nhau giữa hai
máy SenClaw đã ghép cặp qua SLink. App **không** tự nói chuyện với máy kia: daemon
SenClaw ghép cặp, mã hoá, hỏi người dùng rồi gọi `/api/sync/*` của app ở mỗi đầu.
App chỉ quyết định dữ liệu *nghĩa là gì*: hai bản ghi nào là một, bản nào mới hơn,
trường nào không bao giờ rời máy.

| Endpoint | Việc |
|---|---|
| `GET /api/sync/schema` | collection, trường bí mật / trường của máy, khoá ghép, tham chiếu |
| `POST /api/sync/state` | `(id, stamp, deleted)` của từng bản ghi — không có nội dung |
| `POST /api/sync/read` | bản ghi theo id (bí mật chỉ khi `secrets: true`) |
| `POST /api/sync/apply` | nhận bản ghi: bản mới hơn thắng; `dryRun`, `secrets`, `adoptMatches`, `allowRisky` |

- Chỉ nhận request mang token của app (`X-SenClaw-App-Token`, daemon tự đóng dấu);
  không có token trong môi trường (chạy tay `cargo run`) thì guard không chặn gì.
- **Bí mật tắt mặc định.** `password` của host và `value` của keychain không được
  gửi và không được nhận trừ khi cả `read` lẫn `apply` được bật `secrets`. Bí mật
  đã có trên máy nhận không bao giờ bị xoá chỉ vì bên gửi không gửi.
- **Không bao giờ đồng bộ:** `active` của port forwarding (rule nhận về luôn tắt,
  app không mở tunnel vì một peer) và `settings.json` (chính sách lệnh của máy).
- Thay đổi làm credential đi tới nơi khác (đổi `host`/`port`/`user`/`keychain_id`
  của bản ghi đang giữ mật khẩu hoặc keychain, hoặc host mới đã nối sẵn vào
  keychain) bị giữ lại (`held`) cho tới khi người dùng duyệt (`allowRisky`).
- Dữ liệu vẫn nằm ở `hosts.json`, `keychain.json`, `snippets.json`,
  `port_forwarding.json` với đúng định dạng cũ (bản app cũ vẫn đọc được). Stamp và
  bia mộ ở `sync.json` cạnh đó; sửa tay file hoặc sửa bằng bản cũ vẫn được nhận ra
  nhờ so mã băm nội dung. Lần đầu chạy: sao lưu `*.pre-sync.bak`, đặt quyền `0600`.
- Khai báo cho daemon ở khối `sync` trong `senclaw-manifest.json`; test
  `the_manifest_declares_exactly_what_the_code_enforces` giữ nó khớp với mã.

Thử hai bản cục bộ gọi nhau (HOME, cổng và token đều là scratch):

```bash
cargo build && scripts/e2e-sync.sh
```
