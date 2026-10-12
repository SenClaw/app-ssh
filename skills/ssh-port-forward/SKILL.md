---
name: ssh-port-forward
description: Mở, đóng và kiểm tra trạng thái tunnel port forwarding (ssh -L) qua SSH Manager MCP
triggers:
  - "port forwarding"
  - "port forward"
  - "mở port forwarding"
  - "đóng port forwarding"
  - "mở tunnel"
  - "đóng tunnel"
  - "tunnel ssh"
  - "kiểm tra tunnel"
  - "forward cổng"
  - "ssh -L"
  - "open tunnel"
  - "close tunnel"
  - "tunnel status"
---

# Port forwarding qua SSH Manager

Local port forwarding: traffic tới `bind_address:local_port` trên máy này được chuyển qua một SSH host đã lưu tới `destination_address:destination_port` ở phía bên kia — đúng như `ssh -L`.

## Tên tool & nạp schema

Mọi tool nằm trên MCP server **`ssh-manager-mcp`**. Tên đầy đủ luôn là `mcp__ssh-manager-mcp__<tool>`.

Nạp bằng MỘT lệnh ToolSearch:
`select:mcp__ssh-manager-mcp__ssh_list_port_forwards,mcp__ssh-manager-mcp__ssh_start_port_forward,mcp__ssh-manager-mcp__ssh_stop_port_forward,mcp__ssh-manager-mcp__ssh_add_port_forward,mcp__ssh-manager-mcp__ssh_remove_port_forward,mcp__ssh-manager-mcp__ssh_list_hosts`

## Quy trình

1. **Luôn liệt kê trước**: gọi `ssh_list_port_forwards` để lấy `id` chính xác của rule và trạng thái hiện tại. Không đoán `id`.
2. **Mở tunnel**: `ssh_start_port_forward` với `rule_id` (ưu tiên) hoặc `name`.
3. **Đóng tunnel**: `ssh_stop_port_forward` với `rule_id` hoặc `name`. Rule vẫn được giữ lại, chỉ ngừng chạy và trả lại cổng local.
4. **Tạo rule mới**: `ssh_add_port_forward` với `name`, `host_id` (lấy đúng `id` từ `ssh_list_hosts`), `local_port`, `destination_port`; tùy chọn `bind_address` (mặc định `127.0.0.1`), `destination_address` (mặc định `127.0.0.1`), `start` (mặc định `true` — mở luôn).
5. **Xóa rule**: `ssh_remove_port_forward` — tunnel sẽ được đóng trước khi xóa.

## Đọc trạng thái cho đúng

Mỗi rule trả về cả ý muốn của người dùng lẫn thực tế đang chạy:

| Trường | Ý nghĩa |
| --- | --- |
| `status` | `active` = đang forward thật; `error` = KHÔNG forward được (xem `last_error`); `inactive` = đang tắt |
| `active` | `true` chỉ khi `status = active` |
| `running` | Đã bind cổng local (vẫn `true` khi `status = error`, tức cổng đang bị giữ nhưng session SSH hỏng) |
| `enabled` | Ý muốn đã lưu — rule này sẽ tự mở lại khi app khởi động lại |
| `listening_on`, `uptime_seconds`, `connections`, `total_connections` | Bằng chứng tunnel đang hoạt động |
| `last_error` | Lý do cụ thể khi không chạy được |

Quy tắc: **chỉ báo với người dùng là tunnel đang chạy khi `status = "active"`.** `enabled: true` mà `status: "error"` nghĩa là người dùng đã bật nhưng nó đang hỏng — hãy nói rõ `last_error`.

## Lưu ý

- `ssh_start_port_forward` bind cổng local **và** kiểm tra kết nối SSH trước khi trả về, nên "thành công" là thành công thật. Khi lỗi, thông báo nói rõ nguyên nhân (cổng đã bị chiếm, host không tới được, sai thông tin đăng nhập) — hãy sửa nguyên nhân đó thay vì gọi lại y hệt.
- Nếu `ssh_add_port_forward` báo lỗi khi mở, rule **vẫn đã được tạo**. Sửa nguyên nhân rồi gọi `ssh_start_port_forward` với `id` trả về — đừng tạo lại rule mới.
- Rule đang chạy mà SSH session rớt sẽ tự kết nối lại; nếu không được, `status` chuyển sang `error` kèm lý do.
