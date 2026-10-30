# RAM nhỏ, câu trả lời không đổi, không chậm hơn

Trạng thái: bước 0 xong ([mốc](../../reports/lean-memory-baseline-261030.md)); bước 1a qua cổng, đã vào `main`
(`8d20714`, [báo cáo](../../reports/lean-memory-php-261030.md)). Driver Laravel chỉ giữ 32 file đọc gần nhất, đã vào
`main` (`e92e1f5`, [báo cáo](../../reports/lean-memory-laravel-261030.md)). Bước 2: Python xong (`30bbb1b`), JS chưa
sửa ([báo cáo](../../reports/lean-memory-js-python-261030.md)). Cache câu trả lời: driver Python giữ văn bản JSON
(`e826dc4`); lõi giữ câu trả lời dạng văn bản (`5bfe7ca`), người dùng chấp nhận p95 ổn định chậm hơn để lấy RAM
([báo cáo](../../reports/lean-memory-answers-261030.md)). Chờ chọn bước tiếp.

## Ba tiêu chí, đo được

Phân tích tĩnh PHP/Python/JS không thể đúng 100 %: một lời gọi qua tên động (`$this->$method()`) thì không công cụ
nào biết chắc. Vì vậy "chính xác" ở đây là hai bất biến, cả hai kiểm được:

| Tiêu chí | Nghĩa | Cách kiểm |
|---|---|---|
| Chính xác | 1. Mỗi chỗ gọi là `exact`/`likely`, hoặc được liệt kê `unresolved`; không bao giờ bị bỏ im lặng, kể cả khi bộ phân tích hỏng. 2. Đổi cách lưu trữ thì câu trả lời giống hệt từng byte (trừ dòng `Cost:`). | Bộ câu hỏi cố định chạy thật trên repo thật, `diff` trước/sau. Script so sánh nằm ngoài repo (scratchpad, báo cáo), không thành bộ test. |
| Nhanh | Không chậm hơn hôm nay. | p50/p95 của bộ câu hỏi trên; tỉ lệ `timeout` của hook theo ngày trong `scout status` không tăng. |
| Tiết kiệm | RAM giữ lâu dài và đỉnh lúc đọc lần đầu đều giảm. | PSS cả cây server bằng `measure.sh`: lúc trả lời, đỉnh, 90 s sau. Mỗi lần một phép đo, clone trên đĩa. |

## Mốc hôm nay (30/10/2026)

| Repo | RAM giữ | Đỉnh lần đầu |
|---|---|---|
| Laravel 5.278 `.php` (+2.800 JS) | 550–590 MB (igbinary), 770–800 MB (không) | 1,2–1,4 GB / 1,8 GB |
| Python 8.419 `.py` | 505–560 MB | 815 MB |
| TypeScript 6.076 `.ts` | ~370 MB | chưa đo lần đầu thật |
| Rust 18 / 168 `.rs` | 810 MB / 2,5 GB | như mức giữ |

Lõi Rust của scout chỉ ~8 MB. Gần hết RAM là facts nằm trong mảng PHP, dict Python, object JS: facts PHP của repo
trên chỉ 19 MB trên đĩa (96 MB giải nén) nhưng 160–310 MB trong mảng PHP. Ba driver cùng một hình:
`facts[path]` cộng index theo tên (`pathsByName`, `by_name`, `filesByKey`).

## Hướng chọn

Mỗi driver giữ **index** trong RAM (tên → file, class → file khai báo) và đọc **facts từng file từ đĩa khi cần**, có
trần bộ nhớ. Thuật toán trả lời không đổi, chỉ đổi chỗ lấy facts, nên kiểm được bằng `diff` câu trả lời. Contract
chung cho ba driver là một (index trong RAM, facts từng file trong một gói trên đĩa, có trần); phần đọc/giải mã nằm
trong runtime của từng driver vì dữ liệu sống ở đó.

**Cập nhật 30/10, sau khi đo (bước 0):** bước 1 tách thành 1a và 1b. Chỗ gọi, câu hỏi, scope và unit của mỗi file
lưu thành một chuỗi igbinary trong RAM thay vì mảng PHP: 154 MB → 38 MB trên backend, giải mã cả 5.300 file
39–62 ms. Đọc từ đĩa mỗi câu hỏi thì không hợp: `magicCallers` quét mọi chỗ gọi với mọi class có `__call`, tức
mọi model Eloquent. Vì vậy **1a** là nén trong RAM (không đọc đĩa lúc trả lời, không index mới); **1b** (facts trên
đĩa) chỉ làm nếu 1a không đủ. Chuỗi của 1a chính là khối sẽ ghi xuống đĩa ở 1b.

Không chọn chuyển kho facts vào lõi Rust lúc này: đúng tinh thần lõi trung tính nhưng phải viết lại phần suy luận
kiểu đang đọc facts của file khác trực tiếp, rủi ro lớn nhất. Chỉ quay lại hướng đó nếu cổng ở bước 1 không đạt.

## Các bước

| Bước | Nội dung | Kiểm | Rủi ro |
|---|---|---|---|
| [0](phase-00-baseline.md) | Mốc sạch: bộ câu hỏi cố định, câu trả lời mẫu, RAM tách engine/facts | Số đo, file câu trả lời mẫu | Không đổi code |
| [1](phase-01-php-facts-on-demand.md) | PHP: 1a nén facts từng file trong RAM; 1b facts trên đĩa nếu 1a chưa đủ | **Cổng**: `diff` rỗng (đọc và sửa file), p95 và timeout không tăng, PSS giảm | Thấp–vừa |
| [2](phase-02-js-python-facts-on-demand.md) | JS và Python: cùng contract | Như cổng bước 1 | Thấp–vừa |
| [3](phase-03-webpack-resolution.md) | Alias `@/` và import `.vue` không đuôi của webpack/Laravel Mix/Vue CLI | Repo Vue 2: `CustomPagination.vue` ra 11 chỗ import thay vì 0 | Thấp |
| [4](phase-04-first-read-peak.md) | Đỉnh lúc đọc lần đầu | Đỉnh giảm, thời gian đọc lần đầu không tăng quá 10 % | Vừa |
| [5](phase-05-rust.md) | Rust: tắt phân tích trước, tắt rust-analyzer sớm khi rảnh | **Hỏi trước khi đo** (một lượt ~2,5 GB) | Cần anh chọn |
| [6](phase-06-idle-and-docs.md) | Rút thời gian tự tắt khi rảnh; README | Tỉ lệ timeout không tăng | Thấp |

Thứ tự là thứ tự rủi ro. Bước 1 là cổng: không đạt một trong ba tiêu chí thì dừng, không chuyển sang bước 2.

## Tiêu chí xong

- `diff` câu trả lời rỗng trên bộ câu hỏi cố định của từng ngôn ngữ đã đổi.
- p95 và tỉ lệ timeout của hook không tăng.
- Mục tiêu RAM giữ cho phần facts + index: ≤ 50 MB mỗi driver PHP/JS/Python trên các repo ở bảng mốc. Tổng mỗi repo
  còn cộng sàn runtime (PHP ~15 MB, Python ~15 MB, Node ~45 MB mỗi tiến trình) và driver Laravel (68 MB hôm nay,
  xem ở bước 0). Repo một ngôn ngữ dự kiến quanh 100 MB; repo Laravel + JS sẽ cao hơn, số thật lấy từ bước 1–2.
- `cargo clippy`, `php -l`, `node --check`, `py_compile` sạch.

## Cách đo (sau sự cố 30/10)

Mỗi lần chỉ một phép đo nặng; không hàng đợi chạy nền song song. Clone trên đĩa, không trên `tmpfs`. Hỏi trước
mọi phép đo dự kiến quá 2 GB. Xoá clone và `~/.cache/scout/<hash>` của nó sau khi đo.
