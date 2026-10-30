# Cache câu trả lời: lõi và driver Python (30/10/2026)

Plan: [`plan.md`](../plans/261030-lean-memory-exact-answers/plan.md). Bản sao oh-my-pi (6.076 `.ts`) và hermes-agent
(8.419 `.py`), mỗi build một thư mục cache, 40 câu × 2 lượt mỗi vòng, so bằng `compare.py` (câu chạm trần 10 s so tới
người gọi trực tiếp; dòng `Waited:`/`Cost:` bỏ qua).

## Driver Python: giữ câu trả lời dạng văn bản JSON (`e826dc4`)

Driver giữ câu trả lời `callers` dạng list dict tới khi có file đổi; lõi không giữ hộ (driver Python không khai
`cache`). Nay driver giữ văn bản JSON nó gửi đi và gửi lại nguyên văn; chỉ đường constructor đọc lại thành list.

| hermes | Cũ | Mới |
|---|---|---|
| Câu trả lời, 4 vòng | | 160/160 giống |
| Driver Python, RSS sau 2 lượt | 345 MB | **306 MB** |
| p95 ổn định (trung vị 4 vòng theo từng câu) | 347 ms | 363 ms (+4,8 %) |
| p50 ổn định | 27 ms | 30 ms |

Câu 2 (chạm trần 10 s ở lượt đầu) không tính thời gian: lượt sau đi tiếp xa tới đâu tùy lượt đầu dừng ở đâu. Tải máy
lúc đo 4,5–5.

## Lõi: giữ câu trả lời dạng văn bản (`5bfe7ca`, người dùng chọn đổi tốc độ lấy RAM)

Kiểm lại trước khi đưa vào (omp, 1 vòng): 80/80 câu trả lời giống; lõi 122 → **42 MB**; p95 ổn định 44 → 72 ms;
p50 và lượt vừa bật lại không đổi. Bản cài đã build lại.

## Lõi: các phương án đã đo

`DriverPool::answers` giữ mọi câu trả lời của driver khai `cache` (`callers` của PHP, JS, Rust) dạng
`serde_json::Value` tới khi có file đổi, không trần. Trên omp sau 40 câu: 1.340 câu trả lời, 11,3 MB văn bản, ~110 MB
khi đã parse (gấp 10). Trung vị 4 byte; 1 % câu trả lời trên 100 KB giữ 9,2 MB văn bản (lớn nhất 896 KB).

| omp, 4 vòng | Lõi (RSS) | p50 ổn định | p95 ổn định |
|---|---|---|---|
| Hôm nay (`Value`) | 121–122 MB | 24 ms | 41–48 ms |
| Mọi câu trả lời dạng văn bản | 44–45 MB | 23 ms | 75–95 ms |
| Lai: trên 200 cạnh dạng văn bản, còn lại `Value` | 61–64 MB | 21–23 ms | 75–78 ms |

Câu trả lời giống hệt ở mọi phương án. Chậm nằm ở các lần đi cây lớn (câu 12, 38, 24, 18): mỗi lần dùng lại phải
parse các câu trả lời lớn. Muốn vừa gọn vừa nhanh phải đổi đường đi cây sang một kiểu cạnh gọn thay cho `Value`
(`trace.rs`, `output.rs`): thiết kế lại. Bản vá phương án lai có trong scratchpad của phiên (73 dòng, chỉ
`src/driver.rs`).
