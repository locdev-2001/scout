# Phase 2 — Skill cho agent, điều tra MCP không bị defer

## Bối cảnh

- Tool `mcp__scout__callers` nằm trong danh sách deferred tools của Claude Code: agent chỉ thấy tên, phải ToolSearch để nạp schema rồi mới gọi được.
- Lời dặn trong `~/.claude/CLAUDE.md` (do `scout setup` ghi) có trong mọi phiên mà agent vẫn ít dùng. Skill không chắc mạnh hơn, nhưng có hai lợi thế: danh sách skill luôn hiện, và thân skill dạy được cách dùng chi tiết (khi nào dùng `callers`/`locate`, cách đọc `unresolved`, `--depth`).
- Phase này là lớp bổ sung. Kênh chắc chắn vẫn là hook (phase 1).

## File sửa

- `src/cli.rs`: bước cài/gỡ skill trong `setup`/`uninstall`; `status` báo skill đã cài hay chưa.
- Nội dung skill: một file trong repo (ví dụ `agent/skill.md`) được nhúng vào binary như `INSTRUCTIONS`, để `scout update` luôn cài bản mới nhất.

## Việc làm

1. **Tra trước khi làm:** định dạng skill của Claude Code (thư mục, `SKILL.md`, frontmatter `name`/`description`, giới hạn độ dài mô tả) và việc có cho một MCP server luôn được nạp (không defer) hay không. Dùng Context7 hoặc agent `claude-code-guide`; không đoán.
2. **Viết skill `scout`:**
   - Mô tả nhắm đúng ý định: trước khi đổi chữ ký, đổi tên, xoá hay sửa hành vi một hàm/method/class; khi đọc stack trace; khi hỏi "ai gọi X", "đổi cái này ảnh hưởng gì".
   - Thân skill hướng dẫn gọi CLI qua Bash (`scout callers … --compact`, `scout locate path:line`), không phụ thuộc MCP; cách đọc ba mức `exact`/`likely`/`unresolved`; khi nào cần `--depth`.
3. **Cài/gỡ đối xứng, idempotent:** chỉ ghi vào thư mục skill của chính scout; chạy `setup` hai lần không đổi gì; `uninstall` gỡ sạch.
4. **MCP:** nếu Claude Code có cách ghim một server khỏi deferred thì `setup` bật cách đó. Nếu không có thì viết lại mô tả tool `callers` để nói rõ "dùng thay grep khi tìm người gọi", vì mô tả là thứ agent đọc khi tìm tool.

## Nghiệm thu

- Mở một phiên Claude Code mới: skill `scout` có trong danh sách skill; gọi được bằng `/scout`; chụp màn hình.
- `scout setup` chạy hai lần: lần hai không báo thay đổi. `scout uninstall` xong thì thư mục skill không còn.
- Sau 2–3 ngày: chạy lại script đếm theo phiên, so với baseline của phase 0.

## DoD

- [ ] Skill cài/gỡ qua `setup`/`uninstall`, hiện trong phiên mới.
- [ ] Kết luận về MCP deferred có nguồn (tài liệu hoặc thử thật), và đã áp dụng cách tốt nhất có được.
- [ ] Số lần agent tự gọi sau 2–3 ngày được ghi vào báo cáo cạnh baseline (đóng muộn).
- [ ] Gate máy sạch.

## Rollback

`scout uninstall` gỡ skill; revert commit.
