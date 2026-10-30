# Bước 2: JS và Python giữ facts gọn

## Mốc (30/10/2026, bản sao hermes-agent 8.419 `.py`, oh-my-pi 6.076 `.ts`)

RSS từng tiến trình sau warm-up, rồi sau mỗi lượt 40 câu hỏi:

| | Sau warm-up | Sau lượt 1 | Sau lượt 2 |
|---|---|---|---|
| hermes: driver Python | 340 MB | 389 MB | 399 MB |
| hermes: lõi | 16 MB | 38 MB | 55 MB |
| omp: driver JS (heap) | 289 MB (169) | 316 MB (175) | 317 MB (179) |
| omp: lõi | 11 MB | 121 MB | 121 MB |

- Python: `references` (769.288 dict) chiếm 191 MB trong 267 MB facts.
- JS: Node + TypeScript đã 90 MB RSS (heap 21); facts và index ~150 MB heap, chưa phân rã.
- Lõi: cache câu trả lời `callers` (`DriverPool::answers`) không có trần, chỉ xoá khi file đổi: +110 MB sau 40 câu
  trên omp. Ngoài phạm vi bước này; chờ người dùng quyết.

## Python: reference thành tuple

Đo trên `references` của hermes:

| Dạng | RAM | Nạp cache | Lưu cache | Duyệt mọi reference |
|---|---|---|---|---|
| dict (hôm nay) | 191 MB | 514 ms | 736 ms | 31 ms |
| object `__slots__` | 130 MB | 1.187 ms | 2.531 ms | 16 ms |
| một pickle mỗi file | 32 MB | – | – | 254 ms |
| **tuple** | **141 MB** | **315 ms** | 697 ms | **19 ms** |

`__slots__` làm câu đầu sau khi bật lại chậm thêm ~0,7 s; pickle mỗi file chậm gấp 7 khi duyệt (câu hỏi duyệt mọi
reference của nhiều file nhiều lần). Tuple nhỏ hơn và nhanh hơn ở mọi đo.

Làm: một danh sách trường duy nhất (`to`, `name`, `line`, `kind`, `from`, `key`, `owner`, `member`, `module`,
`modules`, `likely`, `implicit`, lấy từ mọi chỗ code đọc), hằng số vị trí, `Reader.add` dựng tuple, mọi chỗ đọc
dùng hằng số; bản sao đổi `kind` qua một helper.

Kiểm: `compare.py` (câu chạm trần 10 s so tới người gọi trực tiếp) trên hermes, 2 lượt × vòng A/B; kịch bản sửa
file trên bản sao (thêm lời gọi, chèn dòng, đổi tên hàm được import ở file khác, hoàn tác).

## JS

Phân rã heap theo trường trước khi chọn cách (`v8.serialize` từng trường, đoạn đo tạm trong worktree). Chỉ khoản trên
~40 MB mới đáng làm.
