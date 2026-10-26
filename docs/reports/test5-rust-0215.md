# codetrace — đợt D vòng 5, driver Rust (26/10, 01:51–02:15)

Repo (chỉ đọc): `refs/codex` @ `ac9db7d8b5` (5.298 `.rs`), `repos/easy-ai-personalization-engine` (PE) @ `2f140c0`, `refs/oh-my-pi` (omp) @ `5d0aaffaf1`.
Bản sao (scratchpad `rust5/`): `pe/` (sửa code), `tiny/` (chép từ vòng 2), `codex-rs/` (chỉ thư mục `codex-rs`, `git init`, `rust-toolchain.toml` đổi `1.95.0` → `stable` vì máy không có 1.95.0 và để tránh rustup tự tải). Output thô: `rust5/out/*.txt`, nhật ký RAM: `rust5/codex-watch.log`.

## Tóm tắt

- **Lỗi vòng 2 đã hết phần lớn:** gọi từ `main.rs`, field/method cùng tên, dòng `use` bị đếm, gọi ngầm `From`/`Deref` (giờ hiện `[implicit …, unresolved]`), chậm sau khi sửa file lớn (4,7 s → 0,9 s). Còn: hàm do macro sinh, chuỗi dừng ở tên macro.
- **Codex thật: driver Rust không bật.** `Cargo.toml` nằm ở `codex-rs/`, không ở gốc git. Mọi câu hỏi Rust rơi về driver chữ: `file#Type::method` báo `Not found`, `file:dòng` trả **0 người gọi im lặng**. `--repo codex-rs` vẫn quay về gốc git. Phần codex dưới đây chạy trên bản sao `codex-rs/` làm gốc.
- **omp: driver-lsp panic** ở `lsp.rs:575` trong lúc warm-up nền. Sau mỗi câu hỏi Rust, rust-analyzer chết trong 10–30 s, nên câu nào cũng khởi động lạnh (20 s, 7,9 GB).
- **Codex (bản sao): quá nặng cho máy 31 GB.** Lần hỏi đầu mất 9 phút 50 s, CPU 1.582 s. rust-analyzer lên 11,4 GB, riêng `driver-lsp` thêm 4,1 GB. Watchdog phải dừng 2 lần khi RAM trống xuống dưới 3,5 rồi 2,5 GB. Hai lần đó `codetrace stop` để lại rust-analyzer mồ côi (~10 GB), phải `kill` theo PID.
- **So với rg:** trong một crate thì codetrace **hơn** rg: `invalid_request` 319 dòng (rg với mẫu `invalid_request(` ra 316, bỏ sót 3 dòng truyền hàm làm giá trị `.map_err(invalid_request)`), `map_fs_error` 11/11 không lẫn 2 hàm trùng tên, `raw_lines` qua `dyn` 57/57, ẩn test đúng. Xuyên crate trên codex thì chỉ **ngang** rg: đủ dòng nhưng gần như toàn `unresolved`, không tách được 2 method trùng tên ở 2 kiểu. File `*_tests.rs` (1.442 file trong codex), `mod testing` và `mod integration_tests` không bị ẩn, nên lọt vào kết quả.
- Đã xác nhận: câu hỏi TS trên omp không bật rust-analyzer (`pgrep` rỗng sau `callers` + `locate` TS). Trên codex việc này không kiểm được vì ở gốc git không có driver Rust.

## Thời gian / RAM

| Repo | Lần đầu (lạnh) | Ấm | RAM |
|---|---|---|---|
| PE thật | 19,2 s (CPU 61 s) | 2 ms–2,3 s (lần đầu cho mỗi symbol 0,2–2,3 s) | rust-analyzer 3,2 GB; `Cost:` 3,3 GB |
| PE bản sao | 25,4 s | 2–3 ms | 3,0 GB |
| tiny | 2,2 s | 1 ms | 656 MB |
| omp, câu TS | 9,8 s | 5 ms | 586 MB, không có rust-analyzer |
| omp, câu Rust | 19,7 s | **không có**: câu nào cũng lạnh 20–21 s (`a driver restarted`) | rust-analyzer 6,9–7,0 GB; `Cost:` 7,9 GB |
| codex bản sao | **589,5 s** (CPU 1.582 s) | 0,3–17 s trong lúc rust-analyzer còn bận (CPU 154 % sau 12 phút); lặp lại 18 ms–3 s | rust-analyzer RSS đỉnh 11,4 GB + `driver-lsp` 4,1 GB; `Cost:` 13,7 GB |
| PE, `pii` không giới hạn depth | 25,2 s lạnh / 84 ms ấm | | 141 node, có tới `main.rs`, 9,8 KB |

## Bảng đích codex (bản sao `codex-rs/`, `--depth 1 --compact`)

Bắn 17 câu song song. Daemon xử lý lần lượt: câu đầu 589 s, các câu sau 0,3–17 s. Cột "test lọt" là số dòng trong `*_tests.rs` hiện như code thật.

| # | Đích (loại) | Sự thật (rg + đọc) | ct | Thiếu / thừa | Nhãn | Cost | So rg |
|---|---|---|---|---|---|---|---|
| c01 | `HistoryCell::copy_source` (method mặc định của trait, gọi qua `dyn`) | 3 dòng + 1 test | 3 + Hidden 1 | — | 2 exact, 1 `unresolved` (trong `tracing::debug!`) | 15,6 s | hơn |
| c02 | `HistoryCell::raw_lines` (53 impl, gọi qua `dyn`) | 77 dòng `.raw_lines(`, 20 trong test | 57 + Hidden 21 | — | exact | 7,9 s | hơn |
| c03 | `AgentMarkdownCell::copy_source` (impl) | như c01 qua trait | 3 | — | 2 likely, 1 unresolved | 11,5 s | ngang |
| c04 | `process_sse` (`tokio::spawn(process_sse(..))`, nhưng fn có `#[cfg(test)]`) | 3, đều trong test | 0 + Hidden 3 | — | — | 1,9 s | hơn |
| c05 | `list_accessible_connectors_from_mcp_tools` (`tokio::join!`, xuyên crate chatgpt→core) | `chatgpt/src/connectors.rs:64` | đúng 1 | — | **unresolved** | 0,3 s | ngang |
| c06 | `utils/string#approx_token_count` (`pub use` qua 2 crate) | 37 dòng | 36 + Hidden 2 | **thừa** `core/src/realtime_context.rs:118` (gọi hàm private cùng tên trong file đó) | 1 exact, **35 unresolved** | 0,6 s | ngang |
| c07 | `aws-auth/src/signing.rs#header_value` (`#[cfg(test)] fn`) | 5, trong `mod tests` | 0 + Hidden 5 | — | — | 0,5 s | hơn |
| c08 | `record_imported_session` (`#[cfg(test)] fn`) | chỉ trong test | 1 hiện + Hidden 7 | **thừa** `detect/sessions/cur_tests.rs:321,333` (file test qua `#[path]`) | exact | 0,3 s | kém |
| c09 | `SessionState::clone_history` (qua `MutexGuard`) | `session/mod.rs:4760` + 5 dòng trong `tests.rs` | 21 dòng | `mod.rs:4760` chỉ `unresolved`; thừa `user_authorization.rs:74` (của `Session`); 19 dòng test lọt | **0 exact**, 21 unresolved | 17,4 s | kém |
| c10 | `Session::clone_history` (async, qua `Arc`) | 116 dòng rg (34 trong `session/tests.rs` + `extension_tools.rs:457` trong `mod tests`) | 82 dòng + Hidden 34; đủ mọi dòng code thật | thừa `mod.rs:4760` (của `SessionState`); 49 dòng test lọt | 61 exact, 21 unresolved | 5,1 s | ngang |
| c11 | `SandboxPolicy::has_full_disk_write_access` (enum) | trộn với c12, phải đọc kiểu từng dòng | 41 dòng | không tách kiểu; 4 dòng test lọt | **0 exact, 41 unresolved** | 0,6 s (lặp lại 3 s, giống hệt) | ngang |
| c12 | `FileSystemSandboxPolicy::has_full_disk_write_access` (cùng tên, struct khác) | như trên | 3 exact (`self.` trong crate) + **cùng 41 dòng unresolved như c11** | không tách kiểu | 3 exact / 41 unresolved | 0,6 s | ngang |
| c13 | `app-server/src/error_code.rs#invalid_request` (dùng rất rộng) | 316 dòng theo mẫu `invalid_request(` (+ 3 dòng `.map_err(invalid_request)` mà mẫu đó bỏ sót) | **319** dòng / 171 symbol | — | 171 exact, 2 unresolved | 0,4 s | hơn |
| c14 | `protocol/src/error.rs#ThreadNotFound@367` (fn do `codex_err_tuple_constructors!` sinh) | 15 dòng `CodexErr::ThreadNotFound(` | `Not found` (12,7 s); tên trần `CodexErr::ThreadNotFound` chạy đúng lúc RAM vượt ngưỡng, bị dừng | thiếu 15/15 | — | — | kém |
| c15 | `exec-server/src/fs_helper.rs#map_fs_error` (`.map_err(f)`, có 3 hàm trùng tên) | 11 dòng | 11/11 | — | exact `[reference]` | (câu lạnh) | hơn |
| c16 | `ExecutorFileSystem::read_file_text` (trait object, xuyên crate) | 84 dòng | 22 + Hidden 62 = 84 | — | **0 exact, 22 unresolved** | 0,4 s | ngang |
| c17 | `stream_output` (`tokio::spawn(stream_output(..))`) | `local_process.rs:496,507` + 2 test | đủ + Hidden 2 | — | exact | 0,6 s | hơn |

Không kiểm vòng này: lời gọi qua generic bound `T: Trait` trên codex (không đủ RAM để hỏi thêm); kết quả vòng 2 trên tiny (`total_area<T: Shape>` đủ) vẫn là tham chiếu.

omp (mỗi câu là một lần khởi động lạnh, xem lỗi N2):

| Đích | Sự thật | ct | Nhận xét |
|---|---|---|---|
| `pi-vfs/src/fs.rs#Fs::for_cleanup` | `fs.rs:1132` | đúng 1 | không lẫn `BlockingFs::for_cleanup` |
| `BlockingFs::for_cleanup` (xuyên crate pi-vfs→pi-builtins, file `sed.rs` 7.300+ dòng) | 7 dòng | 7/7 exact | hơn rg |
| `provider.rs#FileSystem::for_cleanup` (trait) | `fs.rs:439` | đúng | — |
| `Fs::native` (hàm liên kết không có `self`) | 2 dòng thật + 51 test | 2 exact + Hidden 51 | thừa 3 `unresolved` gọi kiểu method: `meta.native()` `stat.rs:1881`, `self.native()` `wc.rs:278`, `metadata.rs:468` |
| `sed.rs#in_place::TempFile::drop` (`impl Drop`) | gọi ngầm khi rời scope | 0 exact + **34 `unresolved`** | toàn là `drop(x)` (`std::mem::drop`, hàm tự do) trong code macOS/Windows bị `cfg` tắt và trong napi vendor; lời gọi hàm tự do không thể gọi method có `self` |
| `BlockingFs::for_cleanup`, không giới hạn depth | | 33 exact, 137 unresolved, 172 node, 20,9 s | 34+34 dòng nhiễu `drop(..)`; node `cut.rs: integration_tests::…`, `host.rs: testing::run_util` (đều `#[cfg(test)] mod`) hiện ở "Further up" |

PE + tiny (kiểm lại vòng 2): xem mục "Lỗi vòng 2".

## Bảng locate (codex bản sao)

| Dòng | Ngữ cảnh | ct | Đúng? | Cost |
|---|---|---|---|---|
| `aws-auth/src/lib.rs:395` | method trong `impl Trait for` khai báo **trong thân** `async fn` test | `tests::credentials_provider_adapter_…::TestCredentialsProvider::credentials` | đúng (innermost) | **25,5 s** (rust-analyzer đang bận) |
| `core/src/exec.rs:266` | trong `tokio::select!` trong `tokio::spawn(async move {..})` | `cancel_when_either` | đúng | 24 ms |
| `protocol/src/error.rs:341` | thân `macro_rules!` | `codex_err_tuple_constructors` | đúng | 76 ms |
| `protocol/src/error.rs:367` | dòng gọi macro sinh fn `ThreadNotFound` trong `impl CodexErr` | `CodexErr` (khối impl) | chấp nhận được, nhưng không có cách nào tới fn được sinh | 1,7 s |
| `core/src/session/turn.rs:1337` | closure `filter(\|item\| { matches!(..) })` lồng trong struct literal | `track_turn_resolved_config_analytics` | đúng | 268 ms |
| `tui/src/app/event_dispatch.rs:1642` | arm `match` cách đầu hàm 1.600 dòng | `App::handle_event` | đúng | 338 ms |
| `tui/src/external_editor.rs:256` | `impl Drop` trong `mod tests` | `tests::EnvGuard::drop` | đúng | 267 ms |

PE: `columns.rs:219` (method trong `impl` do macro sinh) → `profile_columns`. Như vòng 2.

## Kiểm lại lỗi vòng 2

| Lỗi vòng 2 | Kết quả vòng 5 |
|---|---|
| `main.rs` khai báo lại `mod` làm mất người gọi | **Hết.** `router` → `main.rs:25`; `spawn_refresher` → `:22`; `AppState::new` → `:21` (hết 3 `unresolved` nhiễu); `Config::load` đủ 2; `spawn_worker` → `main.rs:24` (bỏ dòng `pub use`); tiny `total_impl`, `Circle::new` đủ |
| Field và method trùng tên (`AppState::pg`) | **Hết.** `pg` 13/13, `redis` 10/10, exact; hỏi `connectors/mod.rs:29` cũng ra method |
| Dòng `use` đếm là người gọi | **Hết ở tầng 1** (`pii` 37 dòng = rg 37). Ở tầng trên vẫn còn node `(file):1` (`bin/personalization_engine.rs`, `models/customer/mod.rs`) |
| Hàm do `macro_rules!` sinh | **Còn.** `columns.rs#fill@219`, `#lacking_from@224` (cú pháp chính tool in ra) → sụp về `profile_columns`, 1 người gọi `(file)`. Tên trần `lacking_from` thì đúng (2 người gọi). Tiny `get_one` → `src/gen.rs#(file)`, 0 người gọi; `get_one@13` → `Not found`. Codex `ThreadNotFound@367` → `Not found` |
| Gọi trong thân macro dừng ở tên macro | **Còn.** `non_empty_object --depth 3` dừng ở `read_column`/`read_json`/`fill_customer`, không có dòng "branches cut"; tiny `helper_inside_macro` dừng ở `make_getter` |
| `From::from` qua `?`/`.into()` | **Hết (đúng mức).** `errors.rs:18`, `:23` hiện `[implicit From, unresolved]` |
| `Deref` ngầm | **Một phần.** Bắt được `(*w)`, nhưng theo tên kiểu: `wrap.rs#Wrapper::deref` và `errors.rs#Wrapper::deref` trả **cùng** 2 dòng (`errors.rs:40`, `wrap.rs:25`), mỗi bên đúng 1, sai 1. Bỏ sót auto-deref `w.ping()` (`wrap.rs:21`) |
| Hỏi ngay sau khi sửa file lớn mất 1–10 s | **Cải thiện rõ** (PE bản sao, rust-analyzer rảnh): thêm chỗ gọi cuối `api/customers.rs` (1.427 dòng) ngay **902 ms**, sau 1 s 3 ms (vòng 2: 4,7 s / 4,3 s); sửa thân hàm 510 ms → 2 ms; thêm file mới + `mod` 2,8 s → 2 ms (vòng 2: 10,1 s); xoá 2 file 3,4 s, sau 1 s 3,9 s, rồi khoảng 1 phút mỗi câu 11–712 ms (không vào cache), sau đó 2 ms. Mọi kết quả đúng (7 → 6 → 7 → 5 người gọi) |

## Lỗi mới theo mức nặng

### Nặng

**N1. Workspace Cargo không nằm ở gốc git thì không có driver Rust, và không có cảnh báo nào.**
- `drivers/rust/driver.json` có `detect: [{file: "Cargo.toml"}]`, `matches_repo` chỉ xét `repo_root.join(file)` (`driver.rs:330-335`). Codex có `codex-rs/Cargo.toml`. `codetrace doctor` chỉ liệt kê generic/javascript/python.
- Hệ quả: `callers 'codex-rs/protocol/src/protocol.rs#SandboxPolicy::has_full_disk_write_access'` → `Not found`, dù `locate codex-rs/protocol/src/protocol.rs:1336` trả đúng tên đó. `callers codex-rs/protocol/src/protocol.rs:1336` → **0 người gọi, không cảnh báo** (thật: hơn 30). Tên trần → 58 dòng `[text call, unresolved]`, trộn 2 kiểu.
- `--repo codex-rs` không giúp được: `main.rs:146-151` đưa `--repo` qua `find_repo_root`, hàm này đi lên tới thư mục có `.git` (`main.rs:92-94`). `status` vẫn báo server của `/refs/codex`.
- Đề xuất: detect `Cargo.toml` ở mọi độ sâu (bỏ qua `target/`, `vendor/`), khởi động rust-analyzer với `rootUri`/`linkedProjects` là thư mục đó. Khi file `.rs` không có driver kiểu thì ghi rõ "Rust: chỉ tìm theo tên".

**N2. `driver-lsp` panic khi warm-up nền, nên mọi câu hỏi Rust thành khởi động lạnh (omp: 20 s và 7,9 GB mỗi câu).**
- Chạy tay `codetrace driver-lsp` (cwd `drivers/rust`, stdin giữ mở 75 s) thì stderr in: `panicked at src/lsp.rs:575:26: end byte index 34 is not a char boundary; it is inside '中'`.
- `dereferences()` (`lsp.rs:570-577`) tính `code[..index]` cho **mọi** byte trước khi xét `byte == b'*'`. Vì vậy chỉ cần một dòng có ký tự ngoài ASCII trong file có nhắc tên kiểu impl `Deref` là panic. Hàm này chạy trong `implicit_uses` (`lsp.rs:1091-1112`) khi warm-up (`warm_up_step`, `lsp.rs:663`) tới một impl `Deref::deref`.
- Bằng chứng: sau mỗi câu, `ps` thấy `[codetrace] <defunct>` và `pgrep rust-analyzer` rỗng sau 10–30 s; câu kế có `Cost: … a driver restarted`; log server không ghi gì.
- Sửa: xét `byte == b'*'` trước (vị trí của `*` luôn là ranh giới ký tự), hoặc lặp bằng `char_indices()`. Ngoài ra, khi driver chết lúc rảnh nên ghi `stderr_tail` (`driver.rs:72,114`) vào log server; hiện chỉ in khi đang có câu hỏi chờ (`driver.rs:269`).
- Codex trong 14 phút chưa gặp lại lỗi này (chưa thấy restart), có lẽ warm-up chưa tới impl `Deref` nào.

**N3. `codetrace stop` trong lúc rust-analyzer đang khởi động lạnh để lại rust-analyzer mồ côi (~10 GB).** Tái hiện 2/2 lần trên codex:
- 01:50:57 stop "done", 01:51:01 PID 2315084 vẫn sống với 9,9 GB.
- 02:11 stop "done", 9 s sau PID 2330403 vẫn sống.
- Phải `kill` theo PID. Đây đúng là đường dẫn tới OOM mà máy từng gặp.
- `cli.rs:178-192` chỉ `kill <pid server>`, không dùng `kill_tree` (`resources.rs:99`). Lúc đó `driver-lsp` đang chặn trong một request tới rust-analyzer nên không đọc stdin để thoát và dọn con.
- Sửa: `stop` dùng `kill_tree`, và/hoặc chạy rust-analyzer với `PR_SET_PDEATHSIG`/process group riêng để nó chết theo driver.

**N4. Codex vượt ngân sách RAM.**
- rust-analyzer tăng đều tới 11,4 GB RSS (02:06:53), cộng `driver-lsp` 4,1 GB RSS (`ps` PID 2317817, ổn định), `Cost:` báo 13,7 GB.
- Lần đầu chờ 589 s. Lúc chạy một câu tên trần (`CodexErr::ThreadNotFound`) thì RAM trống tụt từ 3,1 xuống 2,2 GB (rust-analyzer +860 MB) và watchdog phải dừng.
- 4,1 GB của chính `driver-lsp` đáng xem lại: chỉ mục `mentions`/`lines_by_file` của 5.281 file, `tokens`, `answers` của warm-up.
- Đề xuất: giới hạn warm-up trên repo lớn, có thể đặt `cachePriming.numThreads` thấp hoặc tắt `procMacro` khi RAM thấp, và báo trước "repo lớn: lần đầu ~10 phút, ~15 GB".

**N5. Lỗi trả về từ rust-analyzer bị coi là "không có gì", rồi được cache.** Đây là giả thuyết, có bằng chứng gián tiếp:
- Trên codex, mọi cạnh xuyên crate đều chỉ có nhãn `unresolved`: c05, c06 (35/36), c11 (41/41), c16 (22/22).
- Có cả cạnh trong cùng crate qua `Deref`: `session/mod.rs:4760` `state.clone_history()` là `unresolved` ở **cả** c09 lẫn c10.
- c11 và c12 trả cùng 41 dòng, nghĩa là không tách được 2 kiểu. Hỏi lại: c06 18 ms (lấy từ cache); c11 3,0 s (tính lại) mà vẫn đúng 0 exact / 41 unresolved như lần đầu.
- Ngược lại, cạnh trong crate không qua `Deref` thì đúng exact (c02, c13, c15, c17).
- Lúc đó rust-analyzer vẫn 154 % CPU (đang index).
- Code: `references` lỗi thành `Value::Null` (`lsp.rs:917` `answer.unwrap_or(Value::Null)`); `semanticTokens` lỗi thành mảng rỗng, lưu vào `self.tokens` tới khi file đổi (`fetch_tokens`, `lsp.rs:1114-1131`), nên `token_at` trả None và cạnh bị đánh `unresolved`; câu trả lời vào `answers` (`lsp.rs:1036`).
- Sửa: khi rust-analyzer trả lỗi (`ContentModified`/`RequestCancelled`) hoặc chưa quiescent, hỏi lại hoặc chờ; không cache câu trả lời dựng từ lỗi; không lưu token rỗng do lỗi.

### Vừa

**V1. Test không bị ẩn khi module test không tên `tests`.**
- `symbol_at` (`lsp.rs:758-759`) chỉ xét thư mục `tests`, file `tests.rs` và module tên `tests`.
- Bị lọt:
  - `#[cfg(test)] #[path = "x_tests.rs"] mod tests;`: 1.442 file `*_tests.rs` trong codex. c08 hiện `cur_tests.rs:321,333`; c09 lọt 19 dòng, c10 lọt 49 dòng.
  - `#[cfg(test)] mod testing` (`omp pi-builtins/src/host.rs:1840-1841`).
  - `#[cfg(test)] mod integration_tests` (`cut.rs:1042`, `ls.rs:5479`).
- Đề xuất: đánh dấu test theo thuộc tính `#[cfg(test)]`/`#[test]`/`#[tokio::test]` trên mod/fn (đọc được từ dòng phía trên khai báo), cộng file `*_tests.rs`/`*_test.rs`.

**V2. Cú pháp `file#tên@dòng` cho hàm do macro sinh không dùng được, kể cả với tên chính tool in ra.**
- `resolve()` (`lsp.rs:792-796`) lấy `target()` (macro_item đúng) rồi lại gọi `symbol_at(name_line)`, nên sụp về `macro_rules!` bao ngoài.
- Fn sinh từ ident truyền vào (`make_getter!(get_one, 1)`, `codex_err_tuple_constructors!(ThreadNotFound(..))`) không có "definition mention" nên `Not found`. Đường `workspace/symbol` (`lsp.rs:831-832`) cũng sụp về `(file)`.
- Sửa: với khoá có `@` thì trả nguyên symbol `@`, và nhận dòng gọi macro làm nơi định nghĩa.

**V3. Nhiễu theo tên khi hình dạng lời gọi không khớp.**
- Lời gọi kiểu hàm tự do `drop(x)` bị tính cho method `TempFile::drop` (34 dòng).
- Lời gọi kiểu method `meta.native()` bị tính cho hàm liên kết không có `self` `Fs::native` (3 dòng).
- Phần lớn nằm trong code bị `cfg` tắt (macOS/Windows), nên rust-analyzer trả `unresolved`.
- Lọc tĩnh được: method có `self` thì chỉ khớp `.name(`/`Type::name`; hàm không có `self` thì không khớp `.name(`.

**V4. Gọi ngầm `Deref` khớp theo tên kiểu, không theo kiểu.** Hai `Wrapper` khác nhau trả cùng danh sách. Auto-deref `w.ping()` không được liệt kê.

### Nhẹ

- **L1.** Sau khi xoá file có chỗ gọi, khoảng 1 phút mỗi câu 11–712 ms (không vào cache), rồi về 2 ms. Kết quả vẫn đúng.
- **L2.** `locate` đầu tiên trên một file khi rust-analyzer còn bận mất tới 25,5 s (`aws-auth/src/lib.rs:395`).
- **L3.** Node `(file):1` còn ở tầng trên của cây không giới hạn depth (PE `pii`: `bin/personalization_engine.rs`, `models/customer/mod.rs`).
- **L4.** Lời gọi nằm trong macro (`tracing::debug!`, `assert!`) được đánh `unresolved` dù đúng đích (c01 `agent_message_consolidation.rs:50`).
- **L5.** (Không phải lỗi của codetrace) `codex-rs/rust-toolchain.toml` ghim `1.95.0`. Máy chưa cài bản này, nên rustup 1.29 sẽ tự tải toolchain khi rust-analyzer chạy `cargo metadata` lần đầu. Nên cảnh báo hoặc dùng toolchain sẵn có.

## Phần chạy đúng

- Trait qua `dyn` với 53 impl, method mặc định, impl cụ thể (`likely`), trait xuyên crate trên omp (7/7).
- Hàm trùng tên ở nhiều crate (`map_fs_error` ×3, `invalid_request` ×3): không lẫn.
- Fn làm giá trị (`.map_err(f)`), `tokio::spawn(f(..))`, `#[cfg(test)] fn` (c04, c07), ẩn test trong `mod tests`/`tests.rs`.
- Hàm dùng rất rộng (319 dòng, hơn rg).
- `locate` trên closure lồng, `impl` trong fn, async block, `select!`, arm `match` dài, thân macro: đều đúng.
- Sửa code (PE bản sao): 4/4 kết quả đúng ngay lần hỏi đầu sau khi sửa.

## Dọn dẹp

- Đã `codetrace stop` PE, `pe/`, `tiny/`, omp, codex, `codex-rs/`.
- Phải `kill` theo PID hai rust-analyzer mồ côi do N3.
- Cuối phiên: `pgrep -a rust-analyzer` rỗng, `codetrace status` 0 server.
- Bản sao `rust5/codex-rs/` (103 MB) và `rust5/pe/` nằm trên tmpfs `/tmp`, nên còn tốn RAM tới khi xoá.
