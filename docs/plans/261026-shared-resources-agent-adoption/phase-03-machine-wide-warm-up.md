# Phase 3 — Giới hạn warm-up toàn máy

## Bối cảnh

Mỗi driver tự quyết số worker warm-up cho riêng repo của nó: PHP 3 worker, hoặc `max(3, 2 × nproc/2)` = 12 khi có câu hỏi đang chờ; JS 2 tiến trình con. Năm worktree mở cùng lúc thì số worker nhân năm, dẫn tới RAM cạn và swap đầy. Không có gì giới hạn tổng.

## Thiết kế

Core giữ một **contract trung tính**, mỗi driver tự tuân theo. Không có `if php` trong core.

- Core (Rust) đặt cho mỗi driver biến môi trường `SCOUT_WORK_SLOTS`: thư mục chứa N file slot (`<runtime_directory>/slots/0..N-1`), N mặc định `nproc/2`, chỉnh được bằng `SCOUT_MAX_WORKERS`.
- Một worker warm-up chỉ chạy khi giữ được khoá `flock` trên một slot. Khoá do hệ điều hành nhả khi tiến trình chết, nên không có slot bị kẹt.
- Trước khi xin slot, driver đọc `MemAvailable`. Dưới ngưỡng (mặc định 1,5 GB, chỉnh được qua biến môi trường) thì chờ, không fork.
- Câu hỏi đang chờ được ưu tiên: driver nào có câu hỏi chờ thì xin slot trước. Mỗi driver vẫn tối đa `2 × parallelism` worker như Beru đặt, nhưng không vượt tổng slot của cả máy.
- Windows không có `flock`/`/proc`: dùng `LockFileEx`; không đọc được RAM thì chỉ giới hạn theo slot.
- **Chờ slot không được làm tiến trình chính của driver im lặng.** Core giết driver khi nó "no answer and no work for 20 s" (`src/driver.rs:235`; log ngày 26/10 có đúng dòng này với driver laravel). Vì vậy việc chờ slot hoặc chờ RAM phải nằm trong tiến trình con đã fork (con chờ slot rồi mới làm), hoặc tiến trình chính vẫn trả lời `sync`/`probe` và các câu hỏi trong lúc chờ. Tuyệt đối không chặn vòng lặp chính bằng `flock` hay `sleep`.

## File sửa

- `src/driver.rs`: truyền `SCOUT_WORK_SLOTS` khi khởi động driver; tạo thư mục slot.
- `drivers/php/worker.php` hoặc `fact-store.php`: xin/nhả slot quanh mỗi job warm-up.
- `drivers/javascript/driver.mjs`, `drivers/python/driver.py`: như trên cho tiến trình con rút facts (nếu có).
- `src/hook.rs` / `src/cli.rs`: `scout status` báo slot đang dùng.

## Trade-off cần hỏi trước

Đặt warm-up gấp dưới giới hạn toàn máy nghĩa là, khi nhiều repo cùng warm-up, câu hỏi đầu ở một repo có thể chậm hơn con số Beru đo được khi chỉ có một repo. Trình bày số đo của phase 0 trước khi chốt N.

## Nghiệm thu

Chạy lại kịch bản chuẩn lượt A (lạnh), lấy mẫu như phase 0:
- Số worker warm-up đồng thời toàn máy không lúc nào vượt N.
- Swap không tăng; `MemAvailable` không xuống dưới ngưỡng vì scout.
- Thời gian sẵn sàng của từng worktree, so với baseline (dự kiến chậm hơn khi chạy song song, nhưng tổng không vượt).
- Kiểm `kill -9` một driver đang giữ slot: slot được nhả, driver khác lấy được.
- 5 repo cùng xin slot: `scout logs` không có dòng `restarting` hay `no answer and no work` nào trong suốt kịch bản.

## DoD

- [ ] Ba điều đầu của phần nghiệm thu có số đo trong `docs/reports/`.
- [ ] Slot không bao giờ kẹt sau khi tiến trình chết.
- [ ] Không driver nào bị core khởi động lại vì chờ slot.
- [ ] Một repo đơn lẻ (không có repo khác warm-up) nhanh như baseline (±10 %).
- [ ] Gate máy sạch.

## Rollback

`SCOUT_MAX_WORKERS=0` nghĩa là không giới hạn (hành vi cũ); revert commit.
