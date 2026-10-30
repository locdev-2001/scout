# Bước 5: Rust

## Hiện trạng

Câu trả lời Rust đến từ `rust-analyzer`; nó tự giữ 0,8 GB (18 file) đến 2,5 GB (168 file, vì nạp cả thư viện phụ
thuộc). Scout đã tắt priming, build script, proc-macro. Với repo dưới 1.500 file, scout còn tự phân tích trước mọi
file (`ANALYSE_AHEAD_LIMIT` trong `src/lsp.rs`), có thể là phần lớn của 2,5 GB; chưa đo.

## Lựa chọn (anh chọn)

| Cách | RAM | Chính xác | Nhanh |
|---|---|---|---|
| Tắt phân tích trước | giảm, chưa biết bao nhiêu | không đổi | câu hỏi đầu mỗi hàm chậm hơn |
| Tắt rust-analyzer sau vài phút rảnh, giữ câu trả lời đã có | về ~8 MB khi rảnh | không đổi | câu hỏi sau khi tắt chờ 15–45 s dựng lại |
| Thay rust-analyzer bằng bộ đọc riêng | có thể ~100 MB | **giảm** | nhanh |

Cách thứ ba trái tiêu chí chính xác; không làm nếu anh không yêu cầu.

## Kiểm

Đo trên repo PE, một lượt ~2,5 GB: chỉ chạy sau khi anh đồng ý.
