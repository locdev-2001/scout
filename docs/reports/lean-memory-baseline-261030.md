# Mốc trước khi giảm RAM (30/10/2026)

Plan: [`docs/plans/261030-lean-memory-exact-answers/plan.md`](../plans/261030-lean-memory-exact-answers/plan.md).
Linux, 12 nhân, 31 GB RAM, binary `main` @ `0a8810d`. Mỗi lần một phép đo.

## Repo và bộ câu hỏi

- Bản sao nông `easy-ai-platform-backend` (5.278 `.php`, `vendor` thật, **không có `.env`**) trong scratchpad.
- Repo thật `easy-ai-platform-backend` @ `c8f67781ba`, cây làm việc sạch, có `.env`: chỉ dùng để lưu câu trả lời mẫu.
- 40 câu hỏi sinh bằng seed cố định: 22 method, 4 scope, 4 accessor, 4 `file:line`, 3 class, 1 tên trần có 218
  chỗ gọi, 1 tên trùng nhiều symbol (`handle`), 1 tên không tồn tại. Câu hỏi, câu trả lời (bỏ dòng `Cost:`) và
  script nằm trong scratchpad của phiên, không nằm trong repo.
- Mỗi bộ chạy hai lượt (ngay sau khi bật lại từ cache, và khi ổn định): câu trả lời giống hệt nhau ở cả bản sao lẫn
  repo thật.

## RAM (bản sao, cache igbinary sạch)

| Lượt | Thời gian trả lời | Lúc trả lời | Đỉnh | 90 s sau |
|---|---|---|---|---|
| Đọc lần đầu | 8,7 s | 524 MB | 1.211 MB | 556 MB |
| Bật lại từ cache | 2,2 s | 608 MB | 608 MB | 566 MB |

Phân rã sau khi ổn định (PSS, tổng 564 MB):

| Tiến trình | PSS | Ghi chú |
|---|---|---|
| Driver PHP (cha + con chờ sẵn) | 174 + 164 | cùng ~340 MB trang chung (RSS 348) |
| Node (driver JS, 2.800 file JS/Vue) | 123 | |
| Driver Laravel (PHP) | 73 | |
| Python | 17 | |
| Lõi + driver generic | 13 | |

Trong ~340 MB của driver PHP: engine ~33 MB, autoload Composer ~35 MB (`memory_get_usage` sau
`require vendor/autoload.php` với cùng tham số `-d`), còn lại **~270–290 MB là facts và index**. Facts trên đĩa:
19 MB nén, 96 MB giải nén; `files` ~131 MB, `classes` ~27 MB, danh sách dùng chung ~5 MB khi giải mã riêng từng phần.

## Thời gian bộ 40 câu hỏi

| | p50 | p95 | max |
|---|---|---|---|
| Bản sao, ngay sau khi bật lại | 29 ms | 571 ms | 2.013 ms (câu đầu, gồm bật server) |
| Bản sao, ổn định | 24 ms | 38 ms | 52 ms |
| Repo thật, ngay sau khi bật lại | 32 ms | 390 ms | 2.794 ms |
| Repo thật, ổn định | 26 ms | 50 ms | 62 ms |

## Lỗi chính xác thấy trong lúc lập mốc (chưa sửa)

- Class được lấy từ container `app(ShopifyPendingInstallService::class)` báo 0 người gọi, cả trên repo thật.
- `FormSubmissionObserver` báo 0 người gọi trên repo thật.
- Khi driver Laravel không boot được ứng dụng (bản sao không có `.env`: lỗi kết nối DB), scope và accessor báo 0
  người gọi; cảnh báo có in nhưng chỉ là câu lỗi DB, không nói rằng người gọi qua quy ước Laravel không biết được.
- Đã ghi từ trước: webpack/Laravel Mix (bước 3 của plan).
