# Phase 0 — Đo lại baseline trên binary mới

Mọi số đo ngày 26/10 lúc 16:52 là của binary `6d2c299`. Bảy commit kéo về sau đó (`de3b52b` … `0093d35`) đổi đúng chỗ tốn tài nguyên: PHP lưu facts, 50 file mỗi child warm-up, giải phóng chỉ mục từ, dọn zombie. DoD chỉ có nghĩa khi so với baseline của binary mới.

## Việc làm

1. `./scout --version` (wrapper tự build lại), `scout stop` để tắt mọi server cũ.
2. **Kịch bản chuẩn:** chọn 5 worktree của easy-ai-dev có cùng backend Laravel (ví dụ `repos/easy-ai-platform-backend` và 4 thư mục trong `worktrees/`).
   - Lượt A — lạnh: xoá `~/.cache/scout/<hash>` của 5 thư mục, chạy `scout hook session-start` cho cả 5 gần như cùng lúc, rồi một lệnh grep tên hàm (qua `scout hook before-search`) ở mỗi worktree.
   - Lượt B — ấm: `scout stop`, chạy lại y như lượt A khi cache còn.
3. Trong suốt mỗi lượt, lấy mẫu mỗi 2 giây: tổng PSS các tiến trình scout/driver (`/proc/<pid>/smaps_rollup`), `MemAvailable`, swap dùng, số worker warm-up đang chạy (tiến trình PHP con có `nice` 19), loadavg. Ghi thêm thời điểm mỗi server trả lời được câu đầu tiên.
4. **Bộ symbol đúng đắn:** chọn 10 symbol trên backend: hàm thường, method gọi qua `$this`, static, scope Laravel (`scopeX`), accessor, job `handle`, listener, middleware, method gọi qua facade, một hàm ít người gọi. Lưu `scout callers <symbol> --compact` (bỏ dòng `Cost:`) của worktree gốc vào `docs/reports/` làm đáp án.
5. **Corpus grep:** trích mọi lệnh Bash có `rg|grep|ag|ack` và mọi lệnh Grep tool trong transcript `~/.claude/projects/*/*.jsonl` (kèm `cwd`), lưu ra scratchpad (không commit, có thể chứa dữ liệu riêng). Chạy `scout hook before-search` trên từng lệnh, đếm: có context / không, và lý do nếu đoán được.
6. **Tỉ lệ tự dùng:** chạy lại script đếm theo phiên (MCP, CLI, grep, hook context) cho 14 phiên gần nhất.

Script đo để trong scratchpad; phần đáng giữ (cách chạy, kết quả) ghi vào báo cáo.

## Kết quả cần có

`docs/reports/baseline-resources-<giờ>.md` gồm: bảng PSS đỉnh / swap / `MemAvailable` thấp nhất / số worker đồng thời / thời gian sẵn sàng cho lượt A và B; đáp án 10 symbol; tỉ lệ trúng của corpus; tỉ lệ tự dùng.

## DoD

- [ ] Báo cáo có đủ các số trên, đo trên binary build từ `0093d35` trở đi.
- [ ] Các ngưỡng trong `plan.md` được xem lại theo số đo thật. Nếu binary mới đã đạt ngưỡng nào thì ghi rõ, không làm phần thừa.
