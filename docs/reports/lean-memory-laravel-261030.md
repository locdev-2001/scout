# Driver Laravel: chỉ giữ các file đọc gần nhất (30/10/2026)

Plan: [`plan.md`](../plans/261030-lean-memory-exact-answers/plan.md). Trước đó: [driver PHP](lean-memory-php-261030.md).
Commit `e92e1f5` (`drivers/php/php-source.php`, `drivers/laravel/driver.php`).

## Nguyên nhân

Trên backend thật (có `.env`, app boot được) driver Laravel giữ ~435 MB. Đo từng trường trong tiến trình (heap PHP
374 MB):

| Trường | MB |
|---|---|
| `PhpFileCache` mô hình file đã phân tích (189 file) | **298,5** |
| `classTraits` | 4,6 |
| `wiring` (route, listener, command) | 2,6 |
| `bootRecord` | 2,2 |
| `callersCache`, `scans`, chỉ mục từ | < 1 |

App được boot trong tiến trình con, không nằm trong driver. Một mô hình file (kèm token) to khoảng 24 lần file nguồn;
phân tích lại tốn 0,6 ms (file cỡ vừa) đến 3,4 ms (file lớn nhất). Driver chỉ đọc file để quét token, và giữ kết quả
mỗi lần quét, nên mô hình giữ lại chỉ có ích khi một lần quét khác chạm cùng file.

## Đổi gì

`PhpFileCache` nhận `modelsKept`: số mô hình giữ lại, bỏ cái lâu không dùng nhất trước; mặc định giữ hết như trước
(các tiến trình con trích xuất facts của driver PHP cần thế để suy luận kiểu nhanh). Driver Laravel dùng 32.

## Kiểm (repo thật @ `30b8637b58`, cây làm việc không đổi suốt lúc đo)

2 vòng A/B xen kẽ, mỗi build một thư mục cache, mỗi vòng 2 lượt × 40 câu:

| | Cũ | Mới |
|---|---|---|
| Câu trả lời | | 160/160 giống từng byte |
| Driver Laravel, RSS 60 s sau | 434 MB | **117 MB** |
| p95 lượt đầu sau khi bật lại | 1.378–1.494 ms | 1.386–1.566 ms |
| p95 ổn định | 36 ms | 32–38 ms |
| `scopeForWorkspace` lúc vừa bật lại | 2,6–2,8 s | 2,9–3,1 s |
| accessor `getFormattedDisplayNumberAttribute` lúc vừa bật lại | 268–333 ms | 130–132 ms |

Không chạy kịch bản sửa file: driver Laravel chỉ boot được trên repo thật, và không sửa cây làm việc của người dùng.
Đường sửa file không đổi (`forget()` như cũ); mô hình bị bỏ được đọc lại từ đĩa, và lõi luôn báo file đổi trước khi
hỏi.

## Tổng repo thật (ước tính)

Đo tổng cây với build mới đã bị dừng để không bật/tắt server của người dùng thêm. Ước tính từ các số đã đo: tổng
~832 MB (sau bước driver PHP) − (434 − 117) ≈ **~515 MB**: driver Laravel ~117, driver PHP ~230, Node ~125–175,
Python, lõi, generic ~45.

Đổi `php-source.php` là đổi khoá cache của driver PHP: mỗi repo PHP đọc lại một lần ở lần hỏi đầu.
