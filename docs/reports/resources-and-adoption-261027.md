# Tài nguyên theo số worktree, và agent nhận được scout — báo cáo nghiệm thu

Plan: [`docs/plans/261026-shared-resources-agent-adoption/plan.md`](../plans/261026-shared-resources-agent-adoption/plan.md). Đo ngày 26–27/10/2026 trên Linux, 12 nhân, 31 GB RAM, binary build từ `0093d35` cộng các thay đổi của plan. Máy có tải nền khoảng 2–3 từ các ứng dụng khác trong lúc đo.

Không có test tự động. Mọi số dưới đây đến từ chạy thật: script lấy mẫu `/proc` mỗi 2 giây, `scout hook … --dry-run` trên các lệnh grep thật, và `scout callers` trên repo thật. Script nằm trong scratchpad của phiên làm việc, không commit vì có đường dẫn và lệnh của dự án riêng.

## Kịch bản chuẩn

Năm worktree của cùng một backend Laravel (khoảng 5.200 file PHP, 3.000 file JS/Vue) cùng chạy hook `session-start`, rồi mỗi worktree lặp lại hook `before-search` cho cùng một tên hàm tới khi có danh sách người gọi. Lượt lạnh xoá cache của 5 worktree trước khi chạy; lượt ấm giữ cache.

| Lượt | Đỉnh PSS scout | PSS cuối | `MemAvailable` thấp nhất | Swap | Slot bận tối đa | Load đỉnh | Sẵn sàng (giây) |
|---|---|---|---|---|---|---|---|
| Trước, lạnh | 6.934 MB | 5.914 MB | 15,4 GB | 0 | không giới hạn (40 worker warm-up cùng lúc) | 22,3 | 37–42 |
| Trước, ấm | 8.082 MB | 7.346 MB | 14,9 GB | 0 | không giới hạn | 19,4 | 9–18 |
| Sau, lạnh | 4.581 MB | 4.581 MB | 13,3 GB | 0 | 6 / 6 | 6,4 | 14–28 |
| Sau, ấm | 6.955 MB | 6.696 MB | 11,2 GB | 1 MB | 6 / 6 | 7,3 | 8–18 |
| Sau, 4 worktree mới được seed | 6.512 MB | 4.352 MB | 11,6 GB | 0 | 6 / 6 | 10,3 | 19–24 |

Lượt "sau, lạnh" kết thúc khoảng 20 giây sau khi worktree cuối sẵn sàng, lúc warm-up nền có thể chưa xong; con số RAM ổn định thật là của lượt ấm. Lượt "trước" đo trên một worktree đã bị xoá giữa chừng (`misa-keep-customer-name`); lượt "sau" thay bằng `zalo-workflow-nodes`, cùng repo.

Phân rã RAM ở trạng thái ổn định (4 server, sau warm-up): 8 driver PHP (php + laravel cho mỗi repo) 2.142 MB PSS; 4 tiến trình con rảnh của pool 1.512 MB PSS, trong đó chỉ 409 MB là riêng; 4 driver JS 559 MB; phần còn lại dưới 150 MB. **Mỗi server Laravel giữ khoảng 1,1–1,3 GB cho chính dữ liệu phân tích của nó.** Giới hạn slot và seed không giảm phần này.

## Đúng đắn của seed

Bộ 10 symbol cố định trên backend: hàm public, method private gọi qua `$this`, method protected, static (2), scope Eloquent, accessor, `handle` của job, listener và middleware. Mỗi worktree được hỏi hai lần: một lần khởi động từ cache của worktree anh em (seed), một lần không có cache nào (lạnh). Kết quả `scout callers --compact` của hai lần, bỏ dòng `Cost:`, phải giống hệt.

| Worktree | Khác so với nguồn seed | CPU warm-up PHP: seed / lạnh | Đọc xong toàn repo: seed / lạnh | `diff` |
|---|---|---|---|---|
| `zma-dealers` | 80 file | 3,7 s / 28,5 s | ~12 s / ~18 s | rỗng (277 dòng caller, 10/10 symbol) |
| `backend-agent-rules` | 134 file, có file bị xoá | 4,8 s / 29,0 s | ~13 s / ~18 s | rỗng (277 dòng caller, 10/10 symbol) |
| Worktree thử, tạo riêng rồi gỡ | 3 file | 0,6 s / 28,6 s | ~3 s / ~13 s | rỗng |

Worktree thử dựng đúng trường hợp khó: file A không đổi, file B mà facts của A dựa vào thì đổi kiểu trả về của một method sang một class mới. Bản seed vẫn nối lời gọi `$query->limit()` trong A tới class mới, giống hệt bản lạnh. Sửa một file sau khi seed rồi hỏi lại: lời gọi mới xuất hiện ngay ở câu hỏi đầu tiên.

Câu trả lời đầu tiên của bản seed chậm hơn bản lạnh (khoảng 6 giây so với 2 giây): server nạp 17 MB cache và đồng bộ các file khác trước khi trả lời, còn bản lạnh chỉ đọc những file chứa tên được hỏi. Đổi lại, tổng CPU giảm khoảng 85 %.

Seed cho driver JS và Python **không làm**: JS warm-up trên backend chỉ 4–6 giây với 2 tiến trình, nhỏ so với 28 giây CPU của PHP. Khoá cache của chúng vẫn theo từng worktree.

## Hook `before-search`

Corpus: 20.825 lệnh Bash có `rg`/`grep`/`ag`/`ack`, lấy từ transcript Claude Code trên máy. "Có tên hàm" được xác định độc lập với scout: đoạn tìm kiếm chứa một định danh camelCase, PascalCase có chữ hoa giữa, hoặc snake_case, và thư mục làm việc còn tồn tại (3.555 lệnh).

| | Trước | Sau |
|---|---|---|
| Lệnh có tên hàm được hook nhận ra | 1.187 / 3.555 (33,4 %) | 2.974 / 3.555 (83,7 %) |
| Bỏ sót vì không ra tên | 1.712 | 197 |
| Bỏ sót vì không tìm ra repo | 486 | 197 |

Sửa: tách nhánh `a\|b`, `a|b`, `(a|b)`; bỏ tiền tố `class`, `const`, `new`…; đọc biến shell gán trong cùng lệnh (`B=…; grep x $B/app`) và `~`; `git grep` tìm trong repo của thư mục làm việc; đường dẫn dạng glob tra repo từ thư mục cha gần nhất còn tồn tại. Một câu trả lời cho tên ứng với nhiều symbol không còn bị bỏ chỉ vì một symbol trong đó không có người gọi.

Chạy thật trên backend với server đã sẵn sàng: 5 lệnh mẫu (grep thường, `\|`, `rg` với `|`, `$B/app`, `git grep`) trả về trong 81–241 ms, đều dưới ngân sách 1,5 giây. Lệnh `git grep` cho một method không ai gọi được ghi `not-useful`, đúng như thiết kế. Mỗi lần hook chạy được ghi vào `~/.cache/scout/hooks.log`, và `scout status` in tỉ lệ theo ngày.

Số liệu đầu tiên của `hooks.log` (26/10, gồm cả các lượt đo ở trên): `before-search` 156 lần, 18 % thêm được người gọi. Trong đó `timeout` 57 lần, phần lớn do kịch bản đo hỏi liên tục lúc server còn đang đọc code; `not-useful` 25; `no-name` 22; `not-running` 14 (server vừa được bật, chưa nhận câu hỏi); `no-repo` 9. Như vậy hết giờ lúc warm-up là lý do lớn nhất khiến agent không nhận được người gọi trong những phút đầu của một phiên. `after-edit` từng hỏi cả với file Markdown (10 lần, đều `not-useful`); giờ bỏ qua file không chứa code, cùng bộ lọc với `before-search`.

## Skill và MCP

- `scout setup` cài `~/.claude/skills/scout/SKILL.md`. Skill xuất hiện trong danh sách skill của phiên Claude Code đang chạy ngay sau khi cài. Chạy `setup` lần hai không đổi nội dung file.
- MCP được đăng ký lại bằng `claude mcp add-json` với `"alwaysLoad": true` (tài liệu Claude Code, mục MCP tool search); `~/.claude.json` xác nhận cấu hình. Server MCP gửi thêm `instructions` (nạp cả khi tool bị defer), và mô tả tool `callers` nói rõ "dùng thay grep/rg".
- **Chưa kiểm được** tool có còn bị defer trong một phiên mới hay không: `claude -p` không đăng nhập được (OAuth hết hạn). Cần mở một phiên mới và xem `/context`.
- **Chưa chạy** `scout uninstall`, vì lệnh này gỡ toàn bộ cài đặt đang dùng của máy.

## Pool PHP co giãn

Backend, server đã warm, mỗi câu hỏi có một file vừa đổi, trung bình 5 lần:

| | Câu nhẹ (`--depth 1`) | Câu nặng (toàn cây) | PSS hai driver PHP |
|---|---|---|---|
| Pool 6 tiến trình con rảnh | 91 ms | 143 ms | 864 MB |
| Giữ 1 tiến trình con rảnh | 55 ms | 131 ms | 851 MB |

Không chậm hơn. RAM gần như không giảm, vì tiến trình con rảnh dùng chung bộ nhớ với tiến trình cha.

## Ngân sách RAM và server build cũ

- Đổi build (mtime của binary) rồi hỏi lại backend: server mới khởi động và tắt server build cũ ngay (`stopping server … of an older build` trong log).
- `SCOUT_MEMORY_BUDGET=700`, hai server (scout 837 MB, backend 1.290 MB): khi server scout có file `.busy` thì không bị tắt. Bỏ `.busy`, ở vòng kiểm tiếp theo (30 giây) server scout bị tắt, log ghi `all servers spend 1844 MB, the budget is 700 MB`.
- Slot: `scout hold-slot` giữ, nhả, nhả khi worker bị `kill -9`, thoát khi worker bỏ đi lúc đang chờ, và chờ khi RAM trống dưới ngưỡng. Trên PHP: một child giữ slot bị `kill -9` thì child sau lấy được slot. Trên Python (357 file, `SCOUT_MAX_WORKERS=1`): 4 worker cùng xin, 1 chạy, xong trong khoảng 2 giây. Một repo đơn lẻ lúc warm-up lạnh: tối đa 4 worker giữ slot chạy cùng lúc, ngoài slot chỉ 1 tiến trình (job lưu cache hoặc job boot Laravel).

## Đối chiếu Definition of Done

| Dòng DoD | Kết quả |
|---|---|
| 5 server cùng sống ≤ 2,5 GB, mỗi worktree thêm ≤ +300 MB | **Không đạt.** Ổn định khoảng 6,7 GB, tức 1,1–1,3 GB mỗi server. Ngưỡng đặt trước khi đo, và phần lớn RAM là dữ liệu phân tích mà driver giữ, không phải warm-up. |
| Bật ngân sách: tổng không vượt trần quá một server | Đạt (thử với 700 MB). Ngân sách mặc định 40 % = 12,5 GB thì không có tác dụng với 5 server ở máy này. |
| Swap không tăng, `MemAvailable` ≥ 2 GB | Đạt (swap 0–1 MB, thấp nhất 11,2 GB). |
| Worker warm-up đồng thời ≤ N | Đạt (slot bận ≤ 6). |
| Không driver nào bị core khởi động lại vì chờ slot | Đạt: 29 log server sau ba lượt 5 worktree không có dòng `restarting` hay `no answer and no work`. |
| Một repo đơn lẻ nhanh như trước ±10 % | Chưa đo riêng. |
| Worktree mới sẵn sàng ≤ 15 s | Một mình: đạt (đọc xong 12–13 s). Năm cái cùng lúc: 19–24 s, chưa đạt. |
| Không zombie tồn quá một câu hỏi | Chưa kiểm riêng: lúc warm-up có thời điểm 17–33 zombie, được dọn ở các lượt sau. |
| `diff` rỗng khi seed, kể cả đổi file phụ thuộc | Đạt (3 worktree). |
| Sửa file sau seed phản ánh ngay | Đạt. |
| Hook ≥ 70 % lệnh có tên hàm | Đạt (83,7 %). |
| `scout status` in tỉ lệ trúng | Đạt. |
| Hook không quá 1,5 s + 100 ms | Mẫu chạy thật 81–241 ms. Chưa đo p99 trên toàn corpus (bản dry-run không gọi server). |
| Skill hiện trong phiên mới, setup/uninstall đối xứng | Skill hiện, setup chạy lại không đổi; uninstall chưa chạy. |
| Số lần agent tự gọi sau 2–3 ngày | Đang chờ. Baseline: 1 lần MCP và 15 lần CLI trên 14 phiên. |
| Gate máy | `cargo build --release` sạch; `cargo clippy` chỉ còn 2 cảnh báo có sẵn từ trước (`generic.rs`, `lsp.rs`); `php -l`, `node --check`, kiểm cú pháp Python sạch. `cargo fmt --check` không áp dụng: repo vốn chưa format theo rustfmt. |

Trong lúc thử ngân sách, server backend mang `SCOUT_MEMORY_BUDGET=700` đã tắt một server 82 MB của `easy-ai-dev` thuộc phiên khác; server đó tự bật lại ở câu hỏi kế tiếp. Ngân sách là biến môi trường của server, nên chỉ đặt nó cho phiên thử nghiệm.
