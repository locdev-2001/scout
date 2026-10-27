# Scout là công cụ phân tích tác động, có mặt mỗi khi agent đổi code

Trạng thái: **đã làm, còn kiểm trong phiên mới** (27/10/2026). Kết quả: [`docs/reports/impact-at-change-261027.md`](../../reports/impact-at-change-261027.md). Còn mở: kiểm Bash/Write trong một phiên Claude Code mở sau khi cài (`Stop` đã chạy thật trong phiên làm việc).

## Vì sao

- 1.402 lệnh grep của agent (25–27/10): chỉ khoảng 12–25 % là câu "ai gọi X". Phần còn lại (tìm định nghĩa, chuỗi, key, lọc output, log) không phải việc của scout. Scout không thay grep; nó trả lời "đổi chỗ này ảnh hưởng tới đâu".
- Phiên có refactor sau khi cài skill/MCP: khoảng 105 lần sửa code, trong đó 28 `Edit`, 10 `Write`, 67 lệnh Bash (`python3 - <<EOF`, `sed -i`). Hook `after-edit` chỉ khớp `Edit|MultiEdit`, nên khoảng 72 % lần sửa không đi qua scout.

## Quyết định (người dùng chốt 27/10)

- Định vị: công cụ phân tích tác động (ai gọi, sửa gì ảnh hưởng gì), không thay grep. README, MCP `instructions`, skill và khối hướng dẫn trong `CLAUDE.md` nói đúng điều đó.
- Phủ lúc sửa code dựa trên chính repo (`git`), không dò theo tên tool: mọi cách sửa đều thấy được.
- Cổng cuối lượt **nhắc một lần** bằng `additionalContext` (không `decision: block`). Đây là ngoại lệ có chủ đích cho nguyên tắc "hook không chặn agent": agent đi tiếp một vòng chỉ khi còn hàm đã đổi mà chưa thấy người gọi.

## Thiết kế

- **Ghi nhận theo phiên** (`~/.cache/scout/sessions/<session_id>.json`): với mỗi repo, commit gốc lúc phiên bắt đầu, dấu vân tay các file code chưa commit, các file phiên đã đổi; và các symbol agent đã thấy người gọi.
- **`after-change`** (`PostToolUse` trên `Edit|MultiEdit|Write|Bash|mcp__scout__callers`): so dấu vân tay để biết lệnh vừa rồi đổi file nào; `git diff -U0 <gốc>` cho dòng đổi; một câu `locate file:dòng,dòng…` cho các hàm chứa chúng; hỏi người gọi của hàm chưa thấy và chèn vào phiên. Lệnh `scout callers` (MCP hay CLI) cũng được ghi là đã thấy.
- **`finish`** (`Stop`): diff toàn bộ file phiên đã đổi so với gốc (kể cả đã commit), hàm nào chưa thấy người gọi thì đưa danh sách người gọi vào phiên một lần (`stop_hook_active` chặn lặp). Subagent không có cổng riêng: hook của subagent mang `session_id` của phiên cha, nên nhắc ở đó sẽ bị tính là đã xem mà agent chính không thấy; `Stop` của phiên cha phủ cả thay đổi của subagent.
- **`locate` nhiều dòng** (`file:5,9,40`): lõi hỏi driver một lượt cho cả loạt dòng, trung tính với ngôn ngữ.
- **Đo**: `hooks.log` ghi `after-change` và `finish` (`all-shown`, `reminded`, `no-change`, `timeout`); `scout status` in tỉ lệ.

## Giới hạn biết trước

- Hàm bị xoá hẳn hoặc đổi tên: dòng mới không còn hàm đó, nên chỉ thấy hàm bao quanh chỗ xoá. Người gọi của tên cũ phải hỏi tay.
- File mới (chưa có ở commit gốc) không được hỏi: chưa ai gọi code mới ngoài chính thay đổi đó.
- File mà người dùng đã sửa dở trước phiên (hoặc đổi nhánh giữa phiên): khi agent đụng vào file đó, cả thay đổi cũ trong file cũng được hỏi, vì diff là cả file so với commit gốc. Thà thừa còn hơn sót.
- Thay đổi agent làm trong vòng đi tiếp sau lời nhắc không bị cổng kiểm tới `Stop` của lượt sau (`stop_hook_active`); `after-change` vẫn đưa người gọi trực tiếp.
- Sửa ở repo khác thư mục làm việc qua Bash (`cd /repo-khac && sed -i …`) chỉ thấy khi repo đó đã được phiên ghi nhận (đã sửa bằng `Edit`/`Write` hoặc là thư mục làm việc).

## Definition of Done

- [x] README, MCP `instructions`, mô tả tool `callers`, skill, khối `CLAUDE.md` nói scout là công cụ phân tích tác động, giữ grep cho tìm kiếm văn bản.
- [ ] Sửa code bằng `Edit`, `Write`, `sed -i`, `python3 - <<EOF` trong phiên Claude Code thật: cả bốn cách đều làm hiện danh sách người gọi của hàm vừa đổi.
- [ ] Đổi một hàm có người gọi rồi kết thúc lượt mà chưa xem người gọi: cổng nhắc đúng một lần, liệt kê hàm đó; lượt sau không nhắc lại.
- [x] Commit giữa lượt: cổng vẫn thấy hàm đã đổi.
- [x] Lệnh Bash không đổi file: hook thêm ≤ 100 ms (đo trên backend Laravel).
- [x] `scout status` in số liệu của `after-change` và `finish`.
- [x] Gate máy: `cargo build --release`, `cargo clippy` không thêm cảnh báo.
