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

## Kiểm tra lệnh của AI agent (Settings → MCP Access)

Mọi lệnh agent gửi qua `ssh_execute_command` (và `POST /api/execute`) đi qua
`src/guard/` trước khi tới server. Mặc định (bản cài mới) là chế độ **Smart**:

1. Lệnh đã học (đúng nguyên văn) → chạy ngay.
2. Chương trình trong danh sách "Always ask for" → chờ admin duyệt.
3. Quy tắc cố định (`src/guard/rules.rs`) tách cả dòng lệnh (`;` `&&` `|`, `$(…)`,
   backtick, `sh -c`, `xargs`, ghi file bằng `>`) và chấm từng lệnh con:
   - chỉ đọc (`df`, `docker ps`, `systemctl status`, `qm list`…) → chạy ngay;
   - xoá, ghi đè, dừng/khởi động lại, cài gói, đổi quyền, firewall, ổ đĩa, đọc
     file bí mật (`.env`, `id_rsa`, `/etc/shadow`…) → chờ admin duyệt, không hỏi model;
   - một tham số mà shell sẽ biến đổi (biến `$X`, glob `*`, `$(…)`, `$'…'`), đặt biến
     môi trường ngoài `LANG`/`LC_*`/`TZ`/`TERM`…, hay một chương trình ngoài
     `/bin`, `/usr/bin`… thì không bao giờ được coi là "chỉ đọc": `X=-delete; find / $X`
     sẽ đi tiếp sang bước review.
4. Còn lại → hỏi **model đang chọn trong SenClaw** (qua bridge `llm.request`) và
   **System One** (`/api/decision/ask`) cùng lúc. Chạy khi model nói `low` và System
   One không phản đối; lệnh đó được **tự thêm vào danh sách đã học** (trừ khi có biến,
   glob hay `$(…)`). Mọi trường hợp khác, kể cả không gọi được model hay dòng lệnh có
   comment `#` (có thể nhắm vào model), → chờ admin duyệt. Lệnh do AI học được kiểm tra
   lại theo quy tắc và danh sách "Always ask" hiện tại mỗi lần chạy.

System One chỉ có quyền *đẩy* một lệnh lên admin, không bao giờ tự cho chạy:
đo 2026-10-26 trên checkpoint `multilingual` local, 60 lệnh có nhãn, câu hỏi tốt
nhất chỉ đúng 45/60 và vẫn coi `cat ~/.ssh/id_rsa` là vô hại ở 0.95. Trên 46 lệnh
mà quy tắc không tự quyết được, model `gemini-3.8-flash-medium` không cho qua lệnh
nguy hiểm nào (0/22) và cho qua 22/24 lệnh vô hại; System One (ngưỡng 0.6) chặn
thêm 3 lệnh vô hại, nên kết quả cuối là 19/24. Mỗi lần review mất khoảng 6–7 giây,
nhưng chỉ ở lần đầu, vì sau đó lệnh đã được học.

Lệnh chờ duyệt hiện ở tab **Approvals** (badge + thông báo). **Approve** chạy lệnh
trên đúng server/kết nối agent đã chỉ; **Approve & remember** chạy và thêm đúng lệnh
đó vào danh sách đã học; **Reject** kèm lý do. Agent nhận `NOT RUN YET` + `approval_id`
và đọc kết quả bằng `ssh_command_status` (hoặc `schedule_watch` trên tool đó).
Hàng chờ chỉ ở trong bộ nhớ: sau 1 giờ không ai duyệt thì hết hạn, khởi động lại app
thì mất. Danh sách đã học ở `learned_commands.json` (0600) cạnh `settings.json` và,
như `settings.json`, không đồng bộ qua SLink.

Các chế độ cũ vẫn còn: **Allowlist** giờ yêu cầu *mọi* chương trình trong dòng lệnh
(kể cả `sudo`) nằm trong danh sách và không ghi file (trước đây chỉ xét từ đầu tiên,
nên `ls; rm -rf /` lọt); **Denylist** cũng xét mọi lệnh con.

Giới hạn:
- Guard chặn những lệnh sai hoặc bị cài cắm (prompt injection) mà agent gửi qua tool
  SSH. Nó **không** phải ranh giới chống một tiến trình trên cùng máy gọi thẳng REST
  của app: `/api/guard/*`, cũng như `PUT /api/settings`, không yêu cầu token, nên Bash
  không bị giới hạn của agent có thể tự duyệt, hoặc đọc `hosts.json` rồi SSH thẳng.
  Muốn chặn cả đường đó thì phải khoá Bash của agent bằng sandbox của SenClaw.
- `sftp_read_file` / `sftp_write_file` chưa đi qua guard (đọc `.env` hay ghi
  `authorized_keys` qua SFTP vẫn chạy ngay).
- Lệnh đã duyệt mà chạy quá 10 phút thì bị báo `failed`. Lệnh có thể vẫn đang chạy
  trên server.

```bash
cargo build && cargo build --example fake_sshd && scripts/e2e-guard.sh
```

Script này chạy app trên HOME và cổng tạm, dùng một server SSH giả
(`examples/fake_sshd.rs`, không chạy lệnh nào, chỉ ghi lại lệnh nhận được) và một
SenClaw giả (`scripts/fake-senclaw-bridge.py`).

| Endpoint | Việc |
|---|---|
| `GET /api/guard/approvals` | các yêu cầu, mới nhất trước |
| `POST /api/guard/approvals/:id/approve` | `{remember}` — chạy trên đúng target |
| `POST /api/guard/approvals/:id/reject` | `{note}` |
| `GET`/`DELETE /api/guard/learned` | danh sách đã học / `{command}` |
| `POST /api/guard/check` | `{command}` — chạy thử quyết định, không chạy lệnh và không học |

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
