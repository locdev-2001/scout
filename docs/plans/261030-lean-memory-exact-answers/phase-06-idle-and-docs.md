# Bước 6: thời gian tự tắt và tài liệu

## Thời gian tự tắt

`IDLE_SECONDS` (`src/daemon.rs`) đang là 10 phút. Hôm nay bật lại PHP từ cache mất 2,2–4,7 s, mà hook `prompt` chỉ
chờ 1,5 s, nên rút ngắn bây giờ sẽ tăng timeout. Chỉ rút sau bước 1–2, khi bật lại chỉ còn đọc index; chọn con số
theo thời gian bật lại đo được, kiểm tỉ lệ timeout trong `scout status` vài ngày.

## README

- RAM giữ theo từng ngôn ngữ, và đỉnh lúc đọc lần đầu (hiện chỉ ghi "khoảng 600 MB khi khởi động từ cache").
- `igbinary` là tuỳ chọn ảnh hưởng RAM của PHP; `scout setup` / lệnh kiểm tra máy báo khi thiếu.
- Hỗ trợ webpack/Laravel Mix/Vue CLI sau bước 3.
