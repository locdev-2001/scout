# Phase 5 — Worktree mới dùng lại facts của repo gốc

## Bối cảnh

Từ `87552be`, PHP lưu facts vào `~/.cache/scout/<sha1(repoRoot)>/php-facts`; JS và Python lưu tương tự. Khoá thư mục theo đường dẫn tuyệt đối, nên 15 worktree của cùng một repo là 15 cache và 15 lần warm-up, dù gần như toàn bộ file giống nhau.

Đọc code (`drivers/php/fact-store.php`, `restoreSaved`/`savedKey`):
- Facts lưu theo **đường dẫn tương đối** (`$saved['files'][$path]`), mỗi file kèm `hash` nội dung. Phần phụ thuộc ngoài (vendor) nằm trong `outside`, cũng theo đường dẫn tương đối và hash.
- Khi đọc lại, file nào khác hash thì đi qua `sync()` như một lần sửa file; quá `SAVED_CHANGES_LIMIT` (500) file khác thì đọc lại toàn bộ.
- Nhưng `savedKey()` có `$root` (đường dẫn tuyệt đối): cache của repo gốc luôn bị worktree khác từ chối.
- JS/Python kiểm file bằng `(mtime, size)`. Checkout mới đổi mtime, nên cache seed sang sẽ bị đọc lại hết.

## Hướng làm: seed, không đổi khoá

Không đổi cách khoá sang content-addressed toàn cục. Thay vào đó, **khi worktree chưa có cache thì chép cache của một anh em cùng repo** sang làm điểm khởi đầu, rồi để cơ chế kiểm `hash` + `outside` + `sync()` sẵn có tự đọc lại những file khác. Như vậy dùng lại logic đúng đắn đã có, không phát minh cách mới.

"Anh em cùng repo" là các thư mục có chung `git rev-parse --git-common-dir`. Đã kiểm ngày 26/10: `repos/easy-ai-platform-backend`, `worktrees/zalo-workflow-nodes`, `worktrees/zma-dealers` cùng trỏ về `repos/easy-ai-platform-backend/.git`. Nguồn seed là cache **mới nhất** trong số anh em, không nhất thiết là repo gốc, vì repo gốc có thể chưa bao giờ được warm.

## Gate điều tra (bắt buộc trước khi code)

Trả lời bằng đọc code và chạy thử, ghi vào báo cáo:

0. Cách tìm anh em: duyệt `git worktree list --porcelain` từ `git-common-dir` (rẻ, không cần quét thư mục) rồi tra cache theo `sha1` từng đường dẫn. Repo không phải git (hoặc không có worktree) thì không seed. Dự phòng nếu cách này không dùng được: anh em là thư mục có cùng remote `origin` và cùng hash `composer.lock`.
1. Facts PHP có chứa **đường dẫn tuyệt đối** ở chỗ nào không (tên file trong Reflection, `getFileName()`, đường dẫn trong `classes`, vendor)? Nếu có thì phải chuẩn hoá về tương đối trước khi seed.
2. Facts lấy từ Reflection (class cha, interface, trait, vendor) có phụ thuộc file khác ngoài `outside` không? Nếu một file không đổi nhưng class cha của nó ở file khác đã đổi, `sync()` có làm mới đúng không? Kiểm bằng trường hợp thật: worktree đổi class cha, giữ nguyên class con.
3. `$root` trong `savedKey()` dùng để làm gì? Bỏ nó (thay bằng hash của danh sách file tương đối) có làm sai điều gì không?
4. Worktree có `vendor/` riêng không (cần `composer install` cho mỗi worktree)? `outside` khác thì đọc lại toàn bộ, nên đo xem điều này xảy ra thường xuyên đến đâu.
5. Driver Laravel: facts route/event/command lấy từ app đã boot. Có lưu không, có phụ thuộc đường dẫn không?

**Tiêu chí đi tiếp:** cả 6 câu có câu trả lời và không có câu nào cho thấy seed trả kết quả sai mà cơ chế kiểm hiện có không bắt được. Nếu có thì dừng phase, trình bày cho người dùng.

## Việc làm (sau gate)

1. Core đưa xuống driver một contract trung tính: `SCOUT_SEED_CACHE` = thư mục cache mới nhất của một anh em, chỉ khi repo hiện tại chưa có cache riêng. Core tự tìm anh em (gate câu 0); driver không biết gì về git worktree.
2. PHP: bỏ `$root` khỏi `savedKey()` (thay bằng thứ tương đương không phụ thuộc vị trí, theo kết quả gate 3); nếu không có cache riêng thì đọc từ `SCOUT_SEED_CACHE`.
3. JS/Python: với các entry đọc từ seed thì kiểm bằng hash nội dung thay vì `(mtime, size)`, rồi lưu lại theo cách thường. Làm sau PHP, chỉ khi số đo cho thấy đáng làm.
4. Seed không bao giờ ghi ngược vào cache của repo gốc.

## Nghiệm thu

- Worktree mới của backend (tạo bằng `git worktree add`): thời gian tới câu trả lời đầu tiên, so với lượt A của phase 0.
- **Đúng đắn:** 10 symbol của phase 0 ở worktree được seed và ở cùng worktree warm-up lạnh (xoá cache, tắt seed): `diff` rỗng, bỏ dòng `Cost:`.
- Worktree có thay đổi thật (một branch đổi vài chục file, có đổi class cha): `diff` với warm-up lạnh vẫn rỗng.
- Sửa một file sau khi seed rồi hỏi lại: kết quả phản ánh thay đổi ngay câu đầu.

## DoD

- [ ] Gate điều tra có báo cáo, cả 6 câu đã trả lời.
- [ ] Worktree mới sẵn sàng ≤ 15 s (hoặc ngưỡng chốt lại sau phase 0), có số đo.
- [ ] `diff` rỗng ở cả hai trường hợp (không đổi, có đổi).
- [ ] Gate máy sạch.

## Rollback

Đổi `savedKey` (bump phiên bản) để mọi cache seed bị từ chối; `rm -rf ~/.cache/scout/*`; revert commit.
