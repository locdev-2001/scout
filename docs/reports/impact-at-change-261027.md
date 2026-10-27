# Scout là công cụ phân tích tác động, hook theo thay đổi thật — báo cáo nghiệm thu

Plan: [`docs/plans/261027-impact-at-change/plan.md`](../plans/261027-impact-at-change/plan.md). Đo ngày 27/10/2026 trên Linux, binary build từ `9316bad` cộng các thay đổi của plan.

Không có test tự động. Số dưới đây đến từ transcript Claude Code thật trên máy, từ một bản clone nông (`git clone --depth 1`) của backend Laravel (10.660 file, `vendor` nối từ repo gốc) được sửa thật, và từ hook nhận đúng JSON Claude Code gửi. Script nằm trong scratchpad của phiên, không commit.

## Vì sao đổi định vị

1.402 lệnh grep/rg/Grep của agent từ 25/10 (bỏ phiên phát triển scout), phân loại bằng heuristic:

| Mục đích | Tỉ lệ |
|---|---|
| Lọc output của lệnh khác (`… \| grep`) | 23 % |
| Tìm định nghĩa | 19 % |
| Tìm chuỗi văn bản | 16 % |
| Tìm một tên, chưa rõ ý | 13 % |
| **Tìm lời gọi / chỗ dùng** | **12 %** |
| Log, `.env`, json | 7 % |
| Key, cột, route, hằng | 3 % |
| Liệt kê file, đếm | 2 % |

Scout chỉ trả lời được 12–25 % số lần đó. Agent không né scout: phiên duy nhất đổi method sau khi cài skill/MCP (`a399c38e`) đã gọi `callers` trước cả 6 lần đổi. Chỗ hổng thật là kênh sửa code:

| Phiên `a399c38e` | Số lần | Hook cũ (`Edit\|MultiEdit`) thấy |
|---|---|---|
| `Edit` | 28 | có |
| `Write` | 10 | không |
| Bash `python3 - <<EOF`, `sed -i` | 67 | không |

## Kịch bản một phiên (bản clone backend)

Mỗi bước sửa file thật rồi gửi hook JSON của `PostToolUse` hoặc `Stop`. Thời gian là cả tiến trình hook, server đã ấm.

| Bước | Hook trả gì | Thời gian |
|---|---|---|
| Bash không đổi file (5 lần) | không gì | 41–48 ms |
| `python3 - <<EOF` sửa `ActivityLogQueryService::applyActionFilter` | người gọi `apply` | 2.109 ms (câu đầu sau `git reset`, server đọc lại file), lượt trước 149 ms |
| `sed -i` sửa `TrainingLinkQuotaGuard::release` | 4 người gọi (2 controller, extractor, …) | 237 ms |
| Agent tự hỏi `callers recountWorkspaceWebsiteLinks` (MCP), rồi `sed -i` sửa hàm đó | không gì: đã xem | 36 ms, 168 ms |
| `Write` ghi lại `WorkspaceCounter.php`, đổi trong `next` | người gọi `Product::generateCode` | 219 ms |
| Một script đổi 6 hàm | người gọi của 3 hàm (trần mỗi lần gọi tool) | 526 ms |
| `git commit` giữa lượt | không gì | 53 ms |
| `Stop` | 3 hàm còn lại (`lockUsageRow`, `filterWebsiteDataSourceIds`, `idsAttachedToBot`), mỗi hàm 2 người gọi | 399 ms |
| `Stop` lần nữa với `stop_hook_active: true` | không gì | 18 ms |
| `Stop` của lượt sau | không gì, log `all-shown` | 125 ms |

Cả 10 hàm bị đổi đều tới tay agent đúng một lần, kể cả 3 hàm thuộc thay đổi đã commit trước `Stop`.

Trong chính phiên làm việc này (Claude Code 2.1.308), tool `Edit` thật sửa `WorkspaceCounter::raiseTo` trên bản clone, và transcript nhận `PostToolUse:Edit hook additional context` kèm người gọi `Product::generateCode`. Phiên này nạp cấu hình hook lúc khởi động (matcher cũ `Edit|MultiEdit`), nên lệnh `sed -i` thật trong phiên không bắn hook. Sau khi cài lại hook, chính phiên này nhận `Stop hook additional context` ở lần kết thúc lượt đầu tiên: 12 symbol phiên đã đổi trong `src/hook.rs` kèm người gọi, cộng 18 symbol "not fetched in time" (vượt trần 12). Danh sách dài vì cả hằng, biến thể enum và trường struct đều là symbol. **Bash và Write chưa được kiểm trong một phiên Claude Code mới** (`claude -p` không đăng nhập được, OAuth hết hạn).

## Sự cố phát hiện trong lúc làm

Wrapper `scout` tự `cargo build` khi mã nguồn mới hơn binary. Trong lúc sửa mã nguồn, mọi hook của các phiên đang mở phải chờ build: phiên yona `51f052d1` có 14 lần `PreToolUse:Bash` bị huỷ sau 10 s (`timedOut: true`), phiên này có 19 lần `PostToolUse:Edit` bị huỷ. Wrapper giờ cho hook chạy binary hiện có và build ở nền: hook với binary cũ mất 18 ms, build nền xong và binary được thay.

## Giới hạn

- Hàm bị xoá hẳn hay đổi tên: chỉ hàm bao quanh chỗ xoá được hỏi; người gọi của tên cũ phải hỏi tay.
- File mới (chưa có ở commit gốc của phiên) không được hỏi.
- Lệnh shell sửa file ở repo khác thư mục làm việc chỉ được thấy khi phiên đã từng sửa repo đó bằng `Edit`/`Write`.
- File sinh tự động (ví dụ i18n) đổi thì người gọi của nó cũng được đưa vào: phiên yona nhận người gọi của `resources/js/i18n/generated/vi.ts#messages`.
- File đã sửa dở trước phiên (hoặc đổi nhánh giữa phiên): khi agent đụng vào, thay đổi cũ trong file cũng được hỏi.
- Thay đổi trong vòng đi tiếp sau lời nhắc chỉ được cổng kiểm ở `Stop` của lượt sau; `after-change` vẫn đưa người gọi trực tiếp.
- Không gắn `SubagentStop`: hook của subagent mang `session_id` của phiên cha, nhắc ở đó sẽ đánh dấu đã xem mà agent chính không thấy. `Stop` của phiên cha phủ cả thay đổi của subagent.
- Codex không có hook: chỉ nhận MCP và hướng dẫn trong `AGENTS.md`.

## Đối chiếu Definition of Done

| Dòng | Kết quả |
|---|---|
| README, MCP, skill, `CLAUDE.md` nói đúng định vị | Đạt. `scout setup` đã ghi lại skill, khối `CLAUDE.md`, `AGENTS.md`. |
| `Edit`, `Write`, `sed -i`, `python3 heredoc` đều hiện người gọi | Đạt với JSON hook thật; `Edit` đạt cả trong phiên Claude Code thật. Bash/Write trong phiên mới: chưa kiểm. |
| Cổng nhắc một lần, lượt sau không nhắc lại | Đạt (kịch bản bước 8–10). Trong phiên mới: chưa kiểm. |
| Commit giữa lượt vẫn thấy hàm đã đổi | Đạt. |
| Bash không đổi file ≤ 100 ms | Đạt: 32–48 ms. |
| `scout status` in số liệu `after-change` và `finish` | Đạt (`finish … % all-shown`). Số của ngày 27/10 gồm cả các lượt chạy kịch bản trên bản clone (`backend-try`). |
| Gate máy | `cargo build --release` sạch; `cargo clippy` chỉ còn 2 cảnh báo có sẵn (`generic.rs`, `lsp.rs`). |
