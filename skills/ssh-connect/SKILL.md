---
name: ssh-connect
description: Hướng dẫn kết nối SSH và thực hiện lệnh trên server (SSH Connection Guide)
triggers:
  - "kết nối ssh"
  - "connect ssh"
  - "thực hiện lệnh"
  - "vào server"
---

# Hướng dẫn kết nối SSH (SSH Connection Guide)

Khi người dùng yêu cầu "kết nối SSH", "connect to ssh", hoặc thực hiện lệnh trên một server cụ thể, hãy làm theo các bước sau:

## Tên tool & kiểm tra khả dụng (đọc TRƯỚC khi kết luận thiếu tool)

Mọi tool của skill này nằm trên MCP server **`ssh-manager-mcp`** (app "SSH Manager"). Tên đầy đủ luôn là `mcp__ssh-manager-mcp__<tool>` — không có dạng rút gọn `mcp__ssh__*` hay `mcp__ssh-manager__*`.

- Nạp schema bằng MỘT lệnh ToolSearch duy nhất:
  `select:mcp__ssh-manager-mcp__ssh_list_hosts,mcp__ssh-manager-mcp__ssh_start_connect_id,mcp__ssh-manager-mcp__ssh_start_connect,mcp__ssh-manager-mcp__ssh_execute_command,mcp__ssh-manager-mcp__ssh_close_connect`
  (ToolSearch không phân biệt gạch ngang/gạch dưới, `mcp__ssh_manager_mcp__...` vẫn resolve.)
- Nếu ToolSearch trả 0 kết quả kèm `deferred_total: 0` nghĩa là phiên này KHÔNG có tool MCP nào — đừng kết luận "chưa cài SSH MCP" và đừng tự chạy ssh bằng shell. Hãy báo cho người dùng: hoặc (a) app SSH Manager chưa chạy / MCP chưa đăng ký, hoặc (b) whitelist `allowed_tools` của đoạn chat đang chặn tool MCP (whitelist khác rỗng thì chỉ các tool trong danh sách được hiển thị), rồi dừng lại.

1. **Tìm Host ID**:
   - Sử dụng tool `mcp__ssh-manager-mcp__ssh_list_hosts` để lấy danh sách các server đã được lưu.
   - Tìm server có tên (name) hoặc địa chỉ (host/IP) khớp với yêu cầu của người dùng.
   - Lấy `id` (host_id) của server đó.

2. **Bắt đầu kết nối (Start Connection)**:
   - **Server đã lưu (có trong danh sách ở Bước 1):** dùng tool `mcp__ssh-manager-mcp__ssh_start_connect_id`, truyền tham số `host_id` = đúng giá trị `id` lấy từ `ssh_list_hosts` (KHÔNG phải name hay IP).
   - **Server chưa lưu (người dùng cung cấp trực tiếp IP/user):** dùng tool `mcp__ssh-manager-mcp__ssh_start_connect` với đầy đủ `host` (IP/hostname) và `user`; `port` tùy chọn (mặc định 22), `password` tùy chọn.
   - Cả hai tool đều khởi tạo kết nối SSH và trả về một `connection_id` duy nhất. 
   - **Lưu ý:** Giao diện ứng dụng sẽ tự động mở một tab Terminal mới để người dùng có thể nhìn thấy tiến trình kết nối và các lệnh bạn sắp thực thi!

3. **Thực thi lệnh (Execute Commands)**:
   - Sử dụng tool `mcp__ssh-manager-mcp__ssh_execute_command` với `connection_id` vừa nhận được ở Bước 2 để thực hiện các lệnh shell.
   - Bạn có thể gọi `mcp__ssh-manager-mcp__ssh_execute_command` nhiều lần với cùng một `connection_id` nếu cần chạy nhiều lệnh.
   - Kết quả trả về và lệnh bạn chạy sẽ hiển thị trực tiếp trên tab Terminal của người dùng, giúp họ theo dõi tiến độ một cách trực quan.

4. **Đóng kết nối (Close Connection)**:
   - Khi hoàn thành tất cả các thao tác trên server, sử dụng tool `mcp__ssh-manager-mcp__ssh_close_connect` với `connection_id` đó để đóng kết nối và giải phóng tài nguyên.
