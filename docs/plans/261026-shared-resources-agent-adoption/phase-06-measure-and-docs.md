# Phase 6 — Đo lại, báo cáo, cập nhật docs

## Việc làm

1. Chạy lại đúng kịch bản chuẩn của phase 0 (lượt A lạnh, lượt B ấm, 5 worktree) trên binary có đủ phase 1–5.
2. Chạy lại corpus grep và bộ 10 symbol (so với đáp án phase 0).
3. Viết `docs/reports/resources-after-<giờ>.md`: bảng trước/sau cho từng dòng DoD trong `plan.md`, mỗi dòng ghi số đo thật và đạt/chưa đạt.
4. Cập nhật docs:
   - `docs/plan.md`, mục "Tối ưu tài nguyên máy": bỏ câu "không ghi index ra đĩa"; mô tả cache facts, seed giữa worktree, giới hạn worker toàn máy, ngân sách RAM.
   - `README.md`, "Cách hoạt động": thêm worktree dùng chung facts và giới hạn tài nguyên; "Quản lý": các biến `SCOUT_MAX_WORKERS`, `SCOUT_MEMORY_BUDGET`, tỉ lệ trúng trong `scout status`, skill.
5. Sau 2–3 ngày: đo lại tỉ lệ agent tự gọi, bổ sung vào báo cáo.

## DoD

- [ ] Mọi ô DoD trong `plan.md` có số đo và trạng thái.
- [ ] Docs khớp hành vi thật (đọc lại sau khi sửa: ngày, đường dẫn, con số).
- [ ] `plan.md` chuyển trạng thái thành "xong" (hoặc ghi rõ dòng nào còn mở).
