# Tài nguyên không tăng theo số worktree, và agent thật sự nhận được scout

Trạng thái: **đã làm phase 0–6, còn mở** (27/10/2026). Kết quả và đối chiếu DoD: [`docs/reports/resources-and-adoption-261027.md`](../../reports/resources-and-adoption-261027.md). Còn mở: RAM ổn định khoảng 1,1–1,3 GB mỗi server nên ngưỡng 2,5 GB cho 5 server không đạt; ngân sách mặc định 40 % quá cao để có tác dụng; chưa kiểm MCP còn bị defer trong phiên mới; chưa chạy `uninstall`; số lần agent tự gọi phải đo lại sau 2–3 ngày.

## Hai vấn đề

1. **Tài nguyên tăng theo số worktree.** Mỗi worktree là một server riêng, đọc lại toàn repo, giữ pool PHP riêng. Đo lúc 16:52 ngày 26/10 (binary cũ `6d2c299`): 3 server ≈ 4,1 GB PSS; máy còn 2,5 GB trống, swap 4 GB đầy; 14:00–14:14 có hơn 15 lần server khởi động cho các worktree easy-ai-dev.
2. **Agent không dùng scout.** 14 phiên gần nhất: agent tự gọi MCP 1 lần, tự gõ CLI 15 lần, grep hàng nghìn lần. Hook `before-search` là kênh đưa scout tới agent nhưng bỏ sót pattern `a\|b`, `a|b`, đường dẫn `$B/app`; tool MCP bị Claude Code xếp vào nhóm deferred.

## Mục tiêu

- RAM/CPU của scout tăng theo **số câu hỏi đang chạy**, không theo số worktree đang mở.
- Mỗi lần agent tìm một tên hàm, nó nhận được danh sách người gọi mà không cần tự nhớ gọi scout.
- Không đánh đổi giá trị cốt lõi "chính xác tuyệt đối" trong [`docs/plan.md`](../../plan.md).

## Phase

| # | Phase | Rủi ro | Phụ thuộc | Trạng thái |
|---|---|---|---|---|
| 0 | [Đo lại baseline trên binary mới](phase-00-baseline.md) | — | — | xong |
| 1 | [Hook before-search phủ đủ + log tỉ lệ trúng](phase-01-hook-coverage.md) | thấp | 0 | xong |
| 2 | [Skill cho agent + điều tra MCP không bị defer](phase-02-skill-and-mcp.md) | thấp | — | xong, chờ đo sau 2–3 ngày |
| 3 | [Giới hạn warm-up toàn máy](phase-03-machine-wide-warm-up.md) | thấp | 0 | xong |
| 4 | [Ngân sách RAM cho server, tắt server rảnh lâu nhất, pool co giãn](phase-04-memory-budget.md) | trung bình thấp | 3 | xong |
| 5 | [Worktree mới dùng lại facts của repo gốc](phase-05-seed-worktree-facts.md) | trung bình | 0, có gate điều tra | xong (PHP; JS/Python không làm, lý do trong báo cáo) |
| 6 | [Đo lại, báo cáo, cập nhật docs](phase-06-measure-and-docs.md) | — | 1–5 | xong |

Mỗi phase ship độc lập, có rollback riêng. Phase 2 không phụ thuộc phase nào, làm song song được.

## Quyết định giữ nguyên

- Hook `session-start` bật server sớm (`6d2c299`) là quyết định đã chốt: plan làm nó rẻ đi, không bỏ. Nếu ngân sách RAM buộc phải hoãn bật sớm thì trình bày trade-off cho người dùng chọn.
- Warm-up gấp `max(3, 2 × parallelism)` worker là quyết định perf của Beru (`be71735`, `87552be`). Phase 3 chỉ đặt nó dưới giới hạn toàn máy; nếu cần hạ con số đó thì hỏi trước, kèm số đo.

## Definition of Done (toàn plan)

Plan xong khi **tất cả** dưới đây đúng, mỗi dòng có số đo thật trong `docs/reports/`:

**Tài nguyên** (kịch bản chuẩn ở phase 0: 5 worktree easy-ai-dev cùng mở phiên rồi grep một tên hàm)
- [ ] **Hiệu quả** (ngân sách RAM tắt, 5 server cùng sống): tổng PSS ≤ **2,5 GB**; mỗi worktree thêm vào sau worktree thứ hai tốn ≤ **+300 MB**.
- [ ] **Chặn trần** (ngân sách RAM bật, mở 8 worktree): tổng PSS không vượt ngân sách quá một server.
- [ ] Swap không tăng trong suốt kịch bản; `MemAvailable` không xuống dưới **2 GB** vì scout.
- [ ] Số worker warm-up chạy cùng lúc trên toàn máy ≤ giới hạn đặt ra (mặc định `nproc/2`), kể cả lúc warm-up gấp.
- [ ] Worktree mới của repo đã warm sẵn sẵn sàng trả lời trong ≤ **15 s** (so với baseline đo ở phase 0).
- [ ] Không còn zombie tồn quá một câu hỏi; server rảnh tự tắt đúng hạn.

**Đúng đắn**
- [ ] Bộ 10 symbol cố định (phase 0): `scout callers --compact` ở worktree được seed và ở worktree warm-up lạnh cho kết quả **giống hệt** (`diff` rỗng, bỏ dòng `Cost:`).
- [ ] Sửa một file ở worktree đã seed rồi hỏi lại: kết quả phản ánh thay đổi ngay câu hỏi đầu tiên.

**Agent nhận được scout**
- [ ] Replay corpus lệnh grep thật (phase 1): tỉ lệ lệnh có tên hàm hợp lệ được thêm context tăng từ baseline lên ≥ **70 %**; mọi lệnh không trúng đều có lý do trong log (`no-name`, `no-repo`, `timeout`, `not-useful`).
- [ ] `scout status` in tỉ lệ trúng của hook theo ngày.
- [ ] Skill `scout` xuất hiện trong danh sách skill của một phiên Claude Code mới; `scout setup`/`uninstall` cài/gỡ đối xứng, chạy lại không đổi gì.
- [ ] Sau 2–3 ngày dùng thật, số lần agent tự gọi scout (MCP + CLI) được đo lại bằng cùng script và ghi cạnh baseline (≈ 1 MCP + 15 CLI / 14 phiên). Dòng này không đóng được trong ngày.

**Gate máy** (mọi phase): `cargo build --release`, `cargo clippy`, `cargo fmt --check`, `php -l` cho mọi file trong `drivers/php`, `drivers/laravel`, `node --check drivers/javascript/driver.mjs`, `python3 -m py_compile drivers/python/driver.py`. Không viết test tự động; nghiệm thu bằng chạy thật và ghi kết quả. Commit theo conventional commits, không ghi số phase hay tên plan vào commit message hoặc comment code.

**Docs**
- [ ] `docs/plan.md` mục "Tối ưu tài nguyên máy" bỏ câu "không ghi index ra đĩa" (đã sai từ khi các driver lưu facts) và ghi đúng cơ chế mới.
- [ ] README "Cách hoạt động" và "Quản lý" mô tả giới hạn tài nguyên, chia sẻ facts giữa worktree, skill.
