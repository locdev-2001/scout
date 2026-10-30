# Bước 2: Python và JS (30/10/2026)

Plan: [`phase-02`](../plans/261030-lean-memory-exact-answers/phase-02-js-python-facts-on-demand.md). Bản sao nông
hermes-agent (8.419 `.py`, 4.458 `.ts`) và oh-my-pi (6.076 `.ts`, 720 `.rs`) trong scratchpad, mỗi build một thư mục
cache, từng repo một.

## Cách so

40 câu mỗi repo (34 `file:dòng` trên khai báo, 5 tên trần, 1 tên không có), seed cố định. Hai câu trên hermes chạm
trần 10 s của một câu hỏi, nên cùng một build hai lượt cho "Further up" khác nhau (đi xa tới đâu tùy tốc độ). Với câu
chạm trần ở bất kỳ build nào, chỉ so phần trước "Further up" (đích và người gọi trực tiếp); mọi câu khác so từng
byte. Dòng `Waited:` và `Cost:` là thời gian, không so.

## Python: reference thành tuple (`30bbb1b`)

| | Cũ | Mới |
|---|---|---|
| Câu trả lời, 2 vòng A/B × 2 lượt | | 160/160 giống |
| Kịch bản sửa file (thêm file gọi hàm, chèn dòng, đổi tên hàm được import, hoàn tác) | | 20/20 giống |
| Driver Python, RSS sau 2 lượt | 399 MB | **345 MB** |
| p95 ổn định | 348–804 ms | 314–352 ms |
| Nạp `references` từ cache / duyệt mọi reference | 514 / 31 ms | 315 / 19 ms |

Kịch bản sửa file đổi đúng câu trả lời ở mỗi bước: thêm file → cạnh mới (704 → 705), chèn dòng → dòng 5 thành 8,
đổi tên → tên cũ "Not found", tên mới tìm thấy; hoàn tác → như ban đầu.

## JS: chưa sửa

Driver JS trên omp: 302 MB RSS; Node + TypeScript đã 90 MB (heap 21); heap 167 MB. Facts đọc từ cache (JSON đã
parse) ~117 MB; phần còn lại là index (`filesByKey`, `filesByDeclaration`, `filesByBase`). Facts JS đã có bảng chuỗi
và mảng tham chiếu phẳng (`references` chỉ ~9 MB). Một phép đo trong driver bằng `structuredClone` báo `references`
76 MB: sai, bản sao làm phồng mảng số; không dùng.

Đòn bẩy còn lại: giữ facts mỗi file dạng chuỗi JSON (35 MB thay vì ~117 MB đã parse), parse khi cần (33 µs/file).
Đây đúng kiểu "cả file một khối" đã bị loại ở PHP vì chậm (câu tên trần 40 → 350 ms); muốn giữ tốc độ phải tách
theo khoá tham chiếu như PHP tách theo tên gọi: thiết kế lại lớn trên driver 2.240 dòng. Chưa làm.

## Ngoài phạm vi, đo được

| Khoản | Sau warm-up | Sau 40 câu × 2 | Ghi chú |
|---|---|---|---|
| Lõi Rust, omp | 11 MB | **121 MB** | `DriverPool::answers` giữ mọi câu trả lời `callers` tới khi có file đổi, không trần |
| Lõi Rust, hermes | 16 MB | 55 MB | như trên |
| Driver Python, hermes | 340 MB | 399 MB (cũ) | `answers` trong driver, không trần |
| Driver Rust (chưa chạy rust-analyzer), omp | 92 MB | 92 MB | chỉ mục `mentions` cho 720 file `.rs` |
| Driver JS, hermes | 186 MB | 189 MB | repo Python có 4.458 file TS |

Cache câu trả lời của lõi là một chỗ sửa cho mọi ngôn ngữ (trần theo byte, hoặc giữ câu trả lời dạng chuỗi JSON
thay vì `serde_json::Value`).
