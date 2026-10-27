# Phase 4 — Ngân sách RAM cho server, tắt server rảnh lâu nhất, pool co giãn

## Bối cảnh

- Server tắt khi rảnh 10 phút (`IDLE_SECONDS` trong `src/daemon.rs`), nhưng không có giới hạn tổng: 10 worktree mở trong 10 phút là 10 server cùng sống.
- `last_activity` của server chỉ nằm trong RAM của chính nó (`AtomicU64`); `.info` chỉ có `pid repo`. Từ bên ngoài không biết server nào rảnh lâu nhất.
- Pool PHP fork sẵn `nproc/2` = 6 con rảnh ở mỗi driver. Đo ngày 26/10: con rảnh ở zalo-workflow-nodes chỉ có ~1,3 MB riêng, nhưng ở backend lên ~66 MB riêng mỗi con.
- Tên socket gắn với mtime của binary: build lại thì server mới chạy song song server cũ cho tới khi server cũ rảnh đủ 10 phút.

## Thiết kế

1. **Đưa trạng thái ra ngoài:** `.info` thêm dấu build và trạng thái. Server `touch` `.info` mỗi khi trả lời xong một câu hỏi (mtime = lần hỏi cuối). Trong lúc trả lời, server tạo file `.busy` cạnh socket và xoá khi xong; mtime không bao giờ bằng 0 nên cần tín hiệu bận tường minh như vậy.
2. **Ngân sách do chính server mới thực thi, không phải client:** `ensure_running` được gọi từ hook `session-start`/`before-search`, mà hook không bao giờ được chặn agent. Vì vậy client vẫn chỉ spawn rồi đi. Ngay khi `serve` khởi động (đã tách khỏi terminal), server mới chạy một thread: cộng PSS của mọi server đang sống (cách đọc đã có trong `src/resources.rs`); nếu tổng vượt ngân sách (mặc định 40 % RAM máy, chỉnh bằng `SCOUT_MEMORY_BUDGET`) thì tắt server có `.info` cũ nhất bằng đúng đường của `scout stop` (`kill_tree`). Không bao giờ tắt server có `.busy`, server vừa được hỏi trong 60 s, hay chính nó.
3. **Server của binary cũ:** cũng trong thread đó, server mới tìm các `.info` cùng repo nhưng khác dấu build, và tắt chúng nếu không có `.busy`.
4. **Pool co giãn:** chỉ giữ 1 con fork sẵn lúc rảnh, fork thêm khi một câu hỏi cần nhiều chunk, và không nạp lại các con dư sau khi xong. Đo thời gian câu hỏi trước/sau để chắc fork theo nhu cầu không làm chậm quá mức.

## File sửa

- `src/daemon.rs`: dấu build trong `.info`, touch `.info`, file `.busy`, thread thực thi ngân sách và tắt server build cũ trong `serve`.
- `src/resources.rs`: hàm cộng PSS theo server (dùng lại cách đo `Cost:`).
- `src/cli.rs`: `servers()` trả thêm mốc hỏi cuối; `scout status` in ra.
- `drivers/php/worker.php`: `WorkerPool` giữ tối thiểu 1 con.

## Nghiệm thu

- Mở lần lượt 8 worktree với ngân sách nhỏ (ví dụ 3 GB): tổng PSS không vượt ngân sách quá một server; server bị tắt là cái hỏi lâu nhất; hỏi lại worktree đó thì server bật lại và trả lời đúng.
- Build lại scout rồi hỏi: server build cũ biến mất ngay.
- Câu hỏi nặng (một hàm dùng rộng) và câu hỏi nhẹ: thời gian trước/sau pool co giãn.

## DoD

- [ ] Tổng PSS bị chặn theo ngân sách, có số đo.
- [ ] Không bao giờ tắt server đang trả lời (thử: câu hỏi dài + mở thêm worktree vượt ngân sách).
- [ ] Hook vẫn không chậm thêm: `session-start`/`before-search` khi vượt ngân sách mất thời gian như khi không vượt (đo p99).
- [ ] Không còn hai server cho cùng một repo sau khi build lại.
- [ ] Pool co giãn: câu hỏi nhẹ chậm thêm ≤ 20 ms (p50), RAM rảnh mỗi driver PHP giảm, có số đo.
- [ ] Gate máy sạch.

## Rollback

`SCOUT_MEMORY_BUDGET=0` tắt việc kiểm ngân sách; revert commit.
