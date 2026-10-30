# Bước 4: đỉnh lúc đọc lần đầu

## Hiện trạng

Laravel đọc lần đầu lên 1,2–1,8 GB rồi xuống mức giữ. Đỉnh gồm tiến trình cha gom facts và nhiều worker fork chạy
song song.

## Làm

1. Worker ghi khối facts thẳng vào gói trên đĩa của bước 1–2; cha chỉ gộp index.
2. Số worker cùng lúc tính theo RAM trống (đã có ngưỡng `SCOUT_MIN_AVAILABLE_MB` cho việc bắt đầu đọc).

## Kiểm

Đỉnh giảm; thời gian đến câu trả lời đầu không tăng quá 10 %; `diff` câu trả lời rỗng.
