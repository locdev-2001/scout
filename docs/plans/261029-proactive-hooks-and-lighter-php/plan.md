# scout chủ động, nhẹ hơn, sửa lỗi có kiểm chứng

Trạng thái: xong phần làm trong đêm 29–30/10/2026. Báo cáo: [`docs/reports/night-261029.md`](../../reports/night-261029.md).

## Mục tiêu

1. Agent nhận người gọi ở mọi khoảnh khắc cần, không phải nhớ gọi skill: trước khi sửa, khi prompt nhắc tới code.
2. Người dùng thấy scout làm việc.
3. Giảm RAM và thời gian của câu trả lời đầu, không đổi câu trả lời.
4. Tìm lỗi có thật, tái hiện, sửa, kiểm lại.

## Các bước

| Bước | Nội dung | Kiểm |
|---|---|---|
| 1 | Hook `before-change` (PreToolUse Edit/MultiEdit), hook `prompt` (UserPromptSubmit), `systemMessage` cho người dùng | JSON hook thật trên bản clone backend; phiên `claude -p` mới cho `prompt` |
| 2 | Facts PHP bằng igbinary (lưu và chuyền giữa tiến trình) | PSS và thời gian, driver cũ và mới cùng binary; `diff` câu trả lời |
| 3 | Sửa lỗi: thứ tự không ổn định, panic cắt log, `cargo build` chồng chất, `.info` rỗng, nhận diện tên trong grep, vendor symlink, file làm PHP dừng, `@mixin`, `locate` của driver generic | Mỗi lỗi: tái hiện trên main, sửa, chạy lại cùng kịch bản |
| 4 | Tài liệu: README, skill, khối hướng dẫn agent | `scout setup` ghi lại |

## Tiêu chí xong

- Hook mới có trong `~/.claude/settings.json` sau `scout setup`; `scout status` báo `installed`.
- PSS server backend giảm, câu trả lời giống hệt trên bộ câu hỏi cố định.
- Mỗi lỗi có bằng chứng trước và sau.
- `cargo clippy` sạch, `php -l`, `node --check`, `py_compile` sạch.

## Còn lại

- `PreToolUse:Edit` trong một phiên Claude Code mới (OAuth của `claude -p` hết hạn).
- Theo dõi tỉ lệ `timeout` của `before-change` trong `scout status` vài ngày.
