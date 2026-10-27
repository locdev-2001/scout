# Phase 1 — Hook before-search phủ đủ, có log tỉ lệ trúng

## Bối cảnh

Hook là kênh duy nhất đưa scout tới agent mà không cần agent tự chọn. Chạy thử trên backend lúc server đã sẵn sàng:

| Lệnh | Có context |
|---|---|
| `grep -rn wordSet app` | có |
| `grep -n "function wordSet\|function intentOverlap" …` | không |
| `rg -n "wordSet\|intentOverlap" app` | không |
| `B=/…/backend; grep -rn wordSet $B/app` | không |

Nguyên nhân: `searched_name` ([src/hook.rs](../../../src/hook.rs)) chỉ nhận pattern đúng một tên; đường dẫn có biến shell không được thay giá trị nên không tìm ra repo. Ngoài ra mọi lần hook không thêm gì đều im lặng, nên không phân biệt được "không phải tên hàm" với "server bận, hết giờ".

## File sửa

- `src/hook.rs`: tách tên, thay biến, ghi log.
- `src/cli.rs`: `scout status` đọc log và in tỉ lệ trúng.

## Việc làm

1. **Tách alternation:** pattern có `\|` (grep BRE) hoặc `|` (rg, `grep -E`) → tách từng nhánh, mỗi nhánh qua `searched_name` như hiện nay. Vẫn giữ `MAX_SEARCHED_NAMES` (server trả lời từng câu một).
2. **Biến shell trong cùng lệnh:** khi `shell_words` gặp `NAME=value` đứng riêng thì ghi nhớ; khi gặp `$NAME`/`${NAME}` ở đường dẫn thì thay bằng giá trị đó, không có thì lấy từ biến môi trường của hook (`HOME`…). Biến không biết giá trị thì bỏ qua lệnh đó như hiện nay.
3. **`git grep`:** coi `grep` đứng sau `git` là một lệnh tìm; nơi tìm là repo của `cwd`.
4. **Log kết quả:** mỗi lần hook chạy, nối một dòng JSON vào `<cache_directory>/hooks.log` (`~/.cache/scout`, không đặt ở `/tmp` vì mất khi reboot mà cần so theo ngày): thời điểm, event, repo, tên đã hỏi, kết quả `added | no-name | no-repo | timeout | not-useful | not-running`, số ms. Giới hạn kích thước (cắt nửa đầu khi vượt khoảng 1 MB). Ghi log lỗi thì bỏ qua: hook không bao giờ được chặn agent.
5. **`scout status`:** in theo ngày số lần chạy và tỉ lệ `added`, cùng phân bổ các lý do còn lại.

## Không làm

- Không tăng `SEARCH_BUDGET` (1,5 s). Nếu `timeout` chiếm phần lớn thì đó là việc của phase 3/4 (server bận vì warm-up), không phải của hook.
- Không chặn lệnh grep của agent hay ép agent dùng scout.

## Nghiệm thu

- Chạy lại corpus của phase 0 qua binary mới; so tỉ lệ `added` trước/sau và phân bổ lý do.
- Bốn lệnh mẫu trong bảng trên đều có context.
- Trong một phiên Claude Code thật ở backend, grep `"a\|b"` thấy dòng `PreToolUse:Bash hook additional context: scout: who calls …`; chụp lại transcript.

## DoD

- [ ] Corpus: ≥ 70 % lệnh có tên hàm hợp lệ được thêm context; phần còn lại đều có lý do trong `hooks.log`.
- [ ] `scout status` in tỉ lệ trúng.
- [ ] Hook không bao giờ quá `SEARCH_BUDGET` + 100 ms (đo trên corpus: p99).
- [ ] Gate máy sạch.

## Rollback

Revert commit; xoá `~/.cache/scout/hooks.log` (`scout uninstall` cũng xoá vì nó nằm trong cache).
