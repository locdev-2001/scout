# Bước 0: mốc sạch

## Vì sao

Số "~330 MB cho cặp tiến trình PHP" hôm 30/10 là ước tính: bảng phân rã đo trên cache đã bị lượt tắt igbinary ghi
đè. Cần số sạch và câu trả lời mẫu trước khi đổi gì.

## Làm

1. Clone trên đĩa (không trong scratchpad `tmpfs`): backend Laravel (kèm `vendor` thật), hermes-agent, oh-my-pi,
   vitalnoni-v2 (Vue 2, webpack), easy-ai-personalization-engine (Rust).
2. Bộ câu hỏi cố định mỗi repo, khoảng 30 câu: method gọi nhiều/ít/không ai gọi, class, `file:line`, tên trần,
   component Vue, accessor/scope/facade Laravel. Lưu câu hỏi và câu trả lời (bỏ dòng `Cost:`) làm mẫu.
3. RAM từng tiến trình (PSS, USS, RSS) sau 90 s, cache igbinary sạch.
4. Tách chi phí trong driver PHP: engine + autoload Composer (chạy `require vendor/autoload.php` với cùng tham số
   `-d` của `driver.json`), facts, index dẫn xuất (`pathsByName`, `callersCache`, `cacheUsers`).
5. Xem driver Laravel giữ 68 MB vì gì (nó boot ứng dụng; lỗi PDO `role "forge"` trên clone là dấu hiệu).
6. p50/p95 thời gian trả lời của bộ câu hỏi; tỉ lệ timeout hook 7 ngày gần nhất từ `scout status`.

## Ra

`docs/reports/lean-memory-baseline-261030.md`: bảng số, đường dẫn câu trả lời mẫu (nằm ngoài repo).

## Rủi ro

Chỉ đo. Từng repo một; Rust (một lượt ~2,5 GB) chỉ chạy sau khi được đồng ý ở bước 5.
