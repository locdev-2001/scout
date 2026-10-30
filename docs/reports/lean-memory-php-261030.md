# Cổng bước 1a: driver PHP giữ facts gọn (30/10/2026)

Plan: [`phase-01`](../plans/261030-lean-memory-exact-answers/phase-01-php-facts-on-demand.md). Mốc:
[`lean-memory-baseline-261030.md`](lean-memory-baseline-261030.md). Commit `8d20714` (chỉ `drivers/php/fact-store.php`).

## Đổi gì

- Chỗ gọi của mỗi file lưu thành chuỗi Payload (igbinary nếu có) **theo từng tên gọi**; `scopes` một chuỗi;
  `questions` + `units` một chuỗi (chỉ đọc khi file đổi). Phần còn lại của facts giữ nguyên dạng mảng.
- Đọc qua `facts()`, `sitesCalled()`, `allSites()`, `scopes()`, `questions()`; ghi qua `keep()`; đăng ký index
  qua `index()`. Không chỗ nào khác giải mã.
- Hai index thay cho vòng quét mọi file: tên method → file khai báo (câu hỏi tên trần); class của receiver → file có
  lời gọi trên nó (`__call` của mọi model Eloquent, danh sách class cần mô tả lúc warm-up).
- Cache lưu đúng dạng đã gói: lúc bật lại không gói lại gì.

Hai lần thử trước bị loại vì chậm: gói cả file một chuỗi (câu tên trần 40 → 350 ms; tên `save` có mặt ở hàng nghìn
file phải giải mã cả file), và gói lại lúc khôi phục cache (+0,7 s câu đầu).

## Chính xác

| Kiểm | Kết quả |
|---|---|
| 40 câu, bản sao, 2 lượt × 3 vòng A/B + lần đọc lần đầu | 241 câu trả lời giống từng byte với mẫu |
| 40 câu, repo thật @ `0b00e05696` (có `.env`, driver Laravel boot app), build cũ và mới | giống từng byte, 2 lượt + 2 vòng A/B |
| Kịch bản sửa file, cùng chuỗi trên cả hai build | 20/20 giống nhau |

Kịch bản sửa file đi đúng các đường ghi: thêm lời gọi mới qua kiểu trả về (`getIntegration` 20 → 21,
`hasSmtpConfigured` 1 → 2); chèn 3 dòng đầu file (`image` dòng 38 → 41, đường dịch dòng); đổi kiểu trả về ở file
khác (cạnh qua `hasSmtpConfigured` mất: đường hỏi lại câu hỏi và làm mới chỗ gọi); hoàn tác (về đúng ban đầu).
`Payload` không có igbinary (dùng `serialize`): mã hoá/giải mã ra đúng giá trị.

## Nhanh

Bản sao, 3 vòng A/B xen kẽ, mỗi build một thư mục cache:

| | Câu đầu sau khi bật lại | p95 lượt đầu | p95 ổn định |
|---|---|---|---|
| Cũ | 1,7–2,2 s | 146–193 ms | 60–65 ms |
| Mới | 1,2–1,5 s | 109–135 ms | 33–58 ms |

Repo thật, 2 vòng: p95 lượt đầu 1,88–2,06 s → 1,46–1,77 s; p95 ổn định 56–60 → 43–57 ms; câu đầu ~3 s cả hai.

## RAM

| | Cũ | Mới |
|---|---|---|
| Driver PHP, bản sao (RSS) | 351 MB | **227 MB** |
| Driver PHP, repo thật, cache mới (RSS) | 345 MB | **239 MB** |
| Tổng cây server, bản sao (PSS, 90 s sau) | 556 MB | **437 MB** |
| Đỉnh trong lúc chạy bộ câu hỏi, bản sao | 621 MB | **492 MB** |
| Đọc lần đầu, bản sao: đỉnh / 90 s sau | 1.389 / 584 MB | **1.258 / 479 MB** |
| Tổng cây server, repo thật (PSS) | ~939 MB | **~832 MB** |

Tiến trình con chờ sẵn của driver PHP vẫn dùng chung trang với cha (USS 1–10 MB).

## Chưa đạt, theo thứ tự lớn nhỏ (repo thật)

Mục tiêu ~100 MB mỗi repo **chưa đạt**. Trên repo thật:

1. **Driver Laravel ~435–453 MB** (USS ~430): lớn nhất, chưa đụng tới. Trên bản sao không có `.env` nó chỉ 66–73 MB
   vì không boot được app, nên phần lớn là ứng dụng đã boot hoặc thứ nó giữ sau khi đọc wiring.
2. Driver PHP ~230 MB: engine ~33, autoload Composer ~35, facts class ~37, `pathsByName` ~16, facts đã gói ~51,
   phần meta ~20, index khác.
3. Node (driver JS cho ~2.800 file JS/Vue của backend) ~125–175 MB: bước 2 của plan.
4. Python, lõi, generic: ~45 MB cộng lại.

## Ghi nhận

- `lists`, `listIndex`, `receiverTypeNames` chỉ tăng trong một server sống lâu (như `sharedLists` trước đây).
- `replaceQuestions` gói lại mọi chỗ gọi của file dù chỉ `questions` đổi; chỉ tốn khi sửa file, kịch bản sửa không
  thấy chậm.
- Đổi code driver PHP là đổi khoá cache: mỗi repo PHP đọc lại một lần ở lần hỏi đầu (backend ~9–12 s, đỉnh ~1,3 GB).
  Server backend thật đã được bật lại bằng build mới.
