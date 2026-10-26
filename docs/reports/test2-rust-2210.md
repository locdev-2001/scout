# codetrace — vòng test 2, Rust (25/10, 22:10)

Repo: `repos/easy-ai-personalization-engine` @ `2f140c0` (165 file `.rs`, chỉ đọc). Bản sao để sửa: `scratchpad/rust2/pe-copy`. Repo tí hon tái hiện lỗi: `scratchpad/rust2/tiny` (lib + `main.rs`, trait/generic/`Vec<T>`/Deref/From/macro). Output thô từng lệnh: `scratchpad/rust2/out/*.txt`.

## Tóm tắt

- Phần lõi rất tốt: trait gọi qua `dyn` và generic, `Self::`, `self.`, closure trong iterator, `map(Type::from)`, `tokio::spawn(f(..))`, closure lồng trong `tokio::spawn(async move {..})`, re-export đổi tên (`start as start_migration`), hàm trùng tên ở module/enum khác, `impl Trait for Vec<T>`, ẩn `#[cfg(test)]` đều đúng. `pii` (hàm dùng rộng) 37/37 dòng gọi khớp `rg`. Truy vấn ấm 2–7 ms (`Cost:`), CLI ~50 ms. Không treo lần nào.
- **Hai lỗi nặng, bỏ sót im lặng hoặc sai mà vẫn ghi `exact`:**
  1. Mọi lời gọi từ `src/main.rs` vào module bị mất, vì `main.rs` khai báo lại `mod api; mod customer_store; …` (mỗi file thuộc 2 crate). `api::router` = 0 người gọi, `spawn_refresher` = 0, `AppState::new` = 0, `Config::load` thiếu `main.rs:18`, `spawn_worker` thiếu `main.rs:24`. Cả chuỗi ảnh hưởng của server HTTP dừng ở `router`, không tới `main`.
  2. Field và method cùng tên (`AppState::pg` field + `fn pg()`): codetrace trả tham chiếu tới **field**, gắn `exact`, và bỏ hết 13 chỗ gọi `.pg()` (tương tự `redis`: bỏ 10 chỗ `.redis()`). Hỏi bằng `file:dòng` trong thân method vẫn ra kết quả của field.
- Mức vừa: hàm/method sinh bởi `macro_rules!` không hỏi được ("Not found", hoặc hỏi tên trần thì ra symbol sai với 0 người gọi); gọi trong thân macro dừng ở tên macro; `From::from` qua `?`/`.into()` và `Deref::deref` ngầm bị bỏ im lặng.
- Sửa code: 7 lần sửa, mọi câu trả lời (hỏi ngay, hỏi lại sau 1 s) **giống hệt daemon mới**. Nhưng hỏi ngay sau khi sửa file lớn mất 1–10 s (không phải 8 ms/390 ms như ghi trong plan); sau 1 s chờ vẫn 4,3 s với file 1.400 dòng; sau 10 s thì 2 ms.
- Lần hỏi đầu (lạnh): 27 s trên repo thật, 29–45 s trên bản sao (CPU 70–190 s). rust-analyzer RSS ~3,0–3,3 GB.

## Bảng đích (`callers --depth 1 --compact`, repo thật)

Sự thật = `rg` + đọc code. "Thời gian" = dòng `Cost:` (ấm); CLI cộng thêm ~50 ms khởi động tiến trình.

| # | Đích | Sự thật | codetrace trả | Thiếu / thừa | Nhãn | Thời gian | Kích thước |
|---|---|---|---|---|---|---|---|
| 1 | `clickhouse_client/mod.rs#ClickHouseReader::get_cart_items` (trait, gọi qua `&dyn`) | `section_builders.rs:355` | đúng 1 | — | `exact` (hợp lý: gọi đúng method của trait) | lạnh 26,9 s; lần 2 511 ms | 633 B |
| 2 | `client.rs#ClickHouseClient::get_cart_items` (impl của trait) | `section_builders.rs:355` qua `dyn` | đúng 1 | — | `likely` (đúng) | 2,3 s (lần đầu cho symbol này) | 650 B |
| 3 | `recommendations.rs#get_cart_items` (hàm tự do trùng tên method trait) | `client.rs:167` | đúng 1, không lẫn với method trait | — | `exact` | 284 ms | 666 B |
| 4 | `customer/mod.rs#Customer::find` (`Self::` từ 4 file `impl Customer` khác nhau + `Customer::find`) | 8 dòng: `customer_addresses.rs:33`, `customers.rs:468,994,1217`, `compute.rs:189`, `detail.rs:17`, `merge.rs:46,49` | đủ 8 dòng / 6 symbol; không lẫn 3 hàm `find` khác (`CustomerIdentity`, `CustomerChannel`, `CustomerAddress`) | — | `exact` | 72 ms | 1,3 KB |
| 5 | `ClickHouseReader::compute_batch` (`ch.compute_batch(..).await?` qua `&dyn`) | `compute.rs:72,201,262` | đủ 3 | — | `exact` | 2 ms | 916 B |
| 6 | `transform.rs#web` (truyền như giá trị: `filter_map(transform::web)`) | `fetch.rs:106` | đúng, kiểu `[reference]`; 3 chỗ test bị ẩn | — | `exact` | 2 ms | 613 B |
| 7 | `outbox.rs#run_workspace` (`tokio::spawn(run_workspace(..))` trong `tokio::spawn(async move{..})`) | `outbox.rs:186` | đúng, người gọi `spawn_worker` | — | `exact` | 2 ms | 554 B |
| 8 | `migrate.rs#record_heartbeat` (closure lồng 2 tầng `tokio::spawn(async move { loop {..} })`) | `migrate.rs:270` | đúng, người gọi `start_with` | — | `exact` | 2 ms | 546 B |
| 9 | `migrate.rs#run` (gọi trong closure truyền vào hàm generic `start_with<Work: FnOnce>`) | `migrate.rs:238` | đúng; `--depth 2` lên `api/mod.rs#start_customer_store_migration` qua re-export đổi tên | thừa `customer_store/mod.rs#(file):1` ở tầng 2 (dòng `pub use`) | `exact` | 2 ms | 764 B |
| 10 | `migrate.rs#start` (re-export `start as start_migration`) | `api/mod.rs:380` (+1 test) | đúng | thừa dòng `pub use` `mod.rs:36` như "người gọi" | `exact` | 2 ms | 804 B |
| 11 | `outbox.rs#spawn_worker` (`pub use outbox::spawn_worker`) | `main.rs:24` | chỉ dòng `pub use` `mod.rs:39` | **thiếu `main.rs:24`** (lỗi nặng 1) | `exact` | 2 ms | 517 B |
| 12 | `api/mod.rs#router` | `main.rs:25` (+10 test) | 0 người gọi | **thiếu `main.rs:25`** | — | 2 ms | 314 B |
| 13 | `live_price_snapshots.rs#spawn_refresher` | `main.rs:22` | 0 (kể cả `--tests`) | **thiếu `main.rs:22`** | — | 2 ms | 320 B |
| 14 | `config.rs#Config::load` | `bin/personalization_engine.rs:305`, `main.rs:18` | chỉ bin `pe` | **thiếu `main.rs:18`** | `exact` | 2 ms | 529 B |
| 15 | `connectors/mod.rs#AppState::new` | `main.rs:21` | 0 thật + 3 `unresolved` nhiễu: `api/mod.rs:532` (`Command::new`, trong `#[cfg(target_os="macos")]`), `columns.rs:153` (`Map::new()`), `:195` (`Vec::new()`) trong thân macro | **thiếu `main.rs:21`**, thừa 3 | `unresolved` | 2 ms | 897 B |
| 16 | `connectors/mod.rs#AppState::pg` (method, field cùng tên ở `:16`) | 13 dòng `.pg()`: `sale_builders.rs:73`, `unified_recommendations.rs:188,257`, `webhooks.rs:64`, `api/mod.rs:85,332`, `main.rs:23,24`, `live_price_snapshots.rs:80`, `merge.rs:41`, `split.rs:41`, `sync_api.rs:19`, `sync.rs:23` | symbol nhảy sang **field** (dòng 16); trả `api/mod.rs:503` (`state.pg.as_ref()`), `connectors/mod.rs:29`, `:122` (`pg: Arc::new(..)`) | **thiếu 13/13, thừa 3** (lỗi nặng 2) | `exact` (sai) | 2 ms | 901 B |
| 17 | `AppState::redis` (cùng kiểu) | 10 dòng `.redis()` | 3 tham chiếu field (`api/mod.rs:519`, `connectors/mod.rs:32,124`) | thiếu 10/10, thừa 3 | `exact` (sai) | 2 ms | 759 B |
| 18 | `types.rs#WebEventType::all` (enum; `RfmSegment::all` trùng tên) | `compute_batch.rs:56`, `types.rs:784` (`Self::all()`) | đủ 2, không lẫn `RfmSegment` | thừa `columns.rs:140` `other.keys().all(..)` trong thân macro (`Iterator::all`, gọi kiểu method nên không thể là hàm liên kết không có `self`) | `exact` + 1 `unresolved` | 2 ms | 1,1 KB |
| 19 | `types.rs#RfmSegment::as_str` (8 hàm `as_str` trong repo) | `import.rs:428`, `compute.rs:580` (closure `map(\|s\| s.as_str())`), `repair.rs:634`, `sync.rs:133`, `types.rs:37` | đủ 5 | — | `exact` | 2 ms | 1,1 KB |
| 20 | `types.rs#RfmSegment::default` (`impl Default`, nhiều `default` khác) | `import.rs:428`, `repair.rs:634`, `sync.rs:133` | đủ 3 | — | `exact` | 2 ms | 843 B |
| 21 | `queries.rs#SegmentListItem::from` (`impl From`, `map(SegmentListItem::from)`) | `queries.rs:163` | đúng | — | `exact` | 2 ms | 595 B |
| 22 | `columns.rs#non_empty_object` (chỉ gọi trong thân `macro_rules!`) | sau khi bung macro: `ProfileValues::from_row`, `from_postgres`, `fill` (gọi tiếp ở `values.rs:31`, `migrate.rs:380`, `gate.rs:157`) | 3 dòng trong thân macro, người gọi là tên macro `read_column`/`read_json`/`fill_customer`; `--depth 3` không đi tiếp | chuỗi ảnh hưởng đứt sau tầng 1, không có dòng "branches cut" | `unresolved` | 1–2 ms | 1,0 KB |
| 23 | `columns.rs#merged_json` (thân macro + `pii_writer.rs:161` + 3 test) | `pii_writer.rs:161`; qua macro: `fill`, `lacking_from`; test `:353,355,356` | `pii_writer.rs:161` + 2 dòng macro; test ẩn đúng, `--tests` hiện đủ 3 | thừa dòng `use` `pii_writer.rs:9` (đếm vào `exact`) | `exact`/`unresolved` | 2 ms | 1,4 KB |
| 24 | `lookup_key.rs#not_a_key_sql`, `columns.rs#profile_changes_to_value` (`#[cfg(test)] fn`) | chỉ trong `mod tests` (`lookup_key.rs:190`, `columns.rs:425`) | 0 + "Hidden: 1"; `--tests` hiện đúng dòng | — | `exact` | 2 ms | 355 B |
| 25 | `gate.rs#pii` (hàm dùng rộng) | 37 dòng gọi | đủ 37/37 (4 dòng `rg` thừa là doc comment) | thừa 12 dòng `use …pii` thành "Direct callers" `(file) [reference]`, đếm vào `exact` (49 thay vì 37) | `exact` | 3 ms | 8,2 KB |
| 26 | `WebEventType::fmt` (`impl Display`) | gọi ngầm qua `format!`/`to_string()` | 0 | không có ghi chú nào rằng gọi ngầm không tra được | — | 2 ms | 284 B |

Repo tí hon (`scratchpad/rust2/tiny`):

| Đích | Sự thật | codetrace | Nhận xét |
|---|---|---|---|
| `shapes/mod.rs#Shape::area` | `describe` (`self.area()` trong method mặc định), `total_area<T: Shape>`, `total_dyn(&[Box<dyn Shape>])`, `total_impl(impl Shape)`, `areas_of` (`map(Shape::area)`) | đủ 5, `exact` | đúng |
| `circle.rs#Circle::area` | như trên, qua trait | 4 `likely` + `areas_of` `exact` (items là `Circle` thật) | đúng |
| `Vec::area` (`impl<T: Shape> Shape for Vec<T>`, `mod.rs:24`) | 4 chỗ gọi qua trait | 4 `likely`, loại đúng `areas_of` | đúng; tên symbol `Vec::area` |
| `wrap.rs#Inner::ping` | `use_wrapper` (auto-deref), `use_star` (`(*w).ping()`), `use_arc` (`Arc`), `make_holder` (`cb: Inner::ping`), `closure_var`, `main.rs:8` | đủ 6 | đúng (`main.rs:8` thấy được vì đi qua đường dẫn lib `tiny::wrap::Inner`) |
| `shapes/mod.rs#total_impl` | `main.rs:6` (`mod shapes;` trong main) | **0** | tái hiện lỗi nặng 1 |
| `circle.rs#Circle::new` | `circle.rs:12`, `main.rs:5` | chỉ `:12` | thiếu `main.rs:5` |
| `errors.rs#AppError::from` | `errors.rs:18` (`query()?`), `:22` (`DbError.into()`), `:27` (`map_err(AppError::from)`) | chỉ `:27` | thiếu `?` và `.into()`, im lặng |
| `wrap.rs#Wrapper::deref` | ngầm ở `wrap.rs:21`, `:25` | 0 | im lặng |
| `gen.rs#helper_inside_macro` | `get_one`, `get_two` (sinh bởi `make_getter!`) | `make_getter` `unresolved`, `--depth 2` không đi tiếp | đứt chuỗi |
| `gen.rs#get_one` | `uses_getter`, test | `Not found`; hỏi tên trần `get_one` → trả **`src/gen.rs#(file)`** với 0 người gọi | resolve sai symbol mà không báo |

## Bảng `locate` (repo thật)

| Dòng | Nội dung | codetrace | Đúng? |
|---|---|---|---|
| `migrate.rs:270` | trong `tokio::spawn(async move { loop { .. } })` lồng 2 tầng | `start_with` | đúng |
| `migrate.rs:238` | trong closure truyền cho `start_with` | `start` | đúng |
| `migrate.rs:290` | `}` đóng `start_with` | `start_with` | đúng |
| `migrate.rs:291` | dòng trống giữa hai hàm | `(file)` | hợp lý |
| `migrate.rs:292` | doc comment của `run` | `run` | hợp lý (symbol tính từ doc comment) |
| `columns.rs:153` | thân `macro_rules! fill_customer` | `fill_customer` | hợp lý |
| `columns.rs:204` / `:220` | `impl ProfileValues` lồng trong `macro_rules! profile_columns` | `profile_columns` | chấp nhận được, nhưng không có cách nào tới `ProfileValues::from_row`/`fill` |
| `columns.rs:231` | dòng `profile_columns! {` (gọi macro cấp file) | `(file)` | đúng |
| `client.rs:40` | `#[async_trait]` trước `impl ClickHouseReader for ClickHouseClient` | `ClickHouseClient` (khối impl) | hợp lý |
| `client.rs:167` | trong method impl trait có `#[async_trait]` | `ClickHouseClient::get_cart_items` | đúng |
| `types.rs:784` | trong closure `\|e\| e.as_str() == ..` | `WebEventType::from_str` | đúng |
| `lookup_key.rs:190` | trong `#[test] fn` của `mod tests` | `tests::migration_053_..._test` | đúng |
| `main.rs:24` | thân `main` | `main` | đúng |

Mọi `locate` 54–73 ms CLI, `Cost:` 1–2 ms. Lưu ý: dòng symbol báo ra là dòng đầu doc comment, không phải dòng `fn` (vd `start` báo `:231`, `fn` ở `:237`; `web` báo `:14`, `fn` ở `:15`). Không sai, nhưng ai nhảy tới dòng đó sẽ thấy comment.

## Sửa code trong lúc hỏi (bản sao, `Customer::find` trừ khi ghi khác)

"Ngay" = hỏi ngay sau khi ghi file; "1 s" = hỏi lại sau 1 s (sau lần hỏi ngay); "mới" = giết daemon (`pkill -f 'release/codetrace serve [/]tmp/…/pe-copy'`, xác nhận rust-analyzer đã chết) rồi hỏi. So sánh bỏ dòng `Cost:`.

| # | Sửa | Ngay | 1 s | Daemon mới | Khớp daemon mới |
|---|---|---|---|---|---|
| 1 | thêm method `exists_probe` gọi `Self::find` trong `detail.rs` (daemon vừa ấm) | 1.020 ms | 4 ms | 29,0 s | giống hệt (thêm `detail.rs:23`) |
| 2 | xoá chỗ gọi `merge.rs:49` (thay bằng hàm không tồn tại, code không biên dịch) | 1.080 ms | 592 ms | 30,0 s | giống hệt (còn `merge.rs:46`) |
| 3 | đổi tên `record_heartbeat` → `record_pulse` (định nghĩa + `migrate.rs:270`) | 3.432 ms | 23 ms | 30,4 s | giống hệt; tên cũ → `Not found` (891 ms) |
| 4 | thêm `impl Default for WebEventType` gọi `Self::from_str` (đích `WebEventType::from_str`) | 5.794 ms | 1.084 ms | 38,1 s | giống hệt (thêm `types.rs:829`) |
| 5 | chèn 1 dòng đầu `api/customers.rs` | 3 ms | 3 ms | 31,2 s | giống hệt; `468→469`, `994→995`, `1217→1218`, giữ `#c533`/`#eaef` |
| 6 | sửa chuỗi lỗi trong thân `find_or_error` + chèn 1 dòng trong thân phía trên `Self::find` | 249 ms | 2 ms | 31,4 s | giống hệt; `detail.rs:17→18`, giữ `#8356` |
| 7 | thêm hàm gọi `Customer::find` cuối `api/customers.rs` (1.427 dòng), daemon đã rảnh | 4.696 ms | 2 ms | 35,5 s | giống hệt |
| 8 | như 7, **chỉ** hỏi sau 1 s | — | 4.304 ms | — | đúng (có chỗ gọi mới) |
| 9 | như 7, chỉ hỏi sau 10 s | — | 2 ms (sau 10 s) | — | đúng |
| 10 | thêm method gọi `Self::find` trong `detail.rs` (file nhỏ), daemon rảnh | 1.149 ms | — | — | đúng |
| 11 | thêm file mới `customer/probe_new.rs` + `mod probe_new;` | 10.148 ms | 3 ms | 31,0 s | giống hệt |

Ghi chú: lần 1–4 đo khi daemon mới khởi động chưa lâu (rút dữ kiện nền có thể còn chạy); từ lần 5 chờ rust-analyzer về dưới 20% CPU rồi mới sửa. Không lần nào trả kết quả cũ hay thiếu; giá phải trả là thời gian. Ba lần (`e5`, `e7`, `e11`) sau `pkill` rust-analyzer chưa thoát trong 10 s, nhưng sau đó không còn tiến trình mồ côi.

## Truy vấn không giới hạn `--depth`

| Đích | Kết quả | Thời gian | Kích thước |
|---|---|---|---|
| `gate.rs#pii` | 49 trực tiếp, 106 symbol phía trên trong 39 file, 155 `exact` | 7 ms (`--compact`), 8 ms (cây đầy đủ) | 11,9 KB gọn / 52 KB đầy đủ (288 dòng) |
| `api/mod.rs#handle_anyhow_error` | 72 `exact`, 11 symbol phía trên | 4 ms | 9,5 KB |
| `AppState::pg` | (sai, xem lỗi nặng 2) | 2 ms | 0,9 KB |

Không có gì vượt 60 s. Nhưng mọi cây đều dừng ở `api/mod.rs#router` (không tới `main.rs#main`), và có node `(file)` sinh từ dòng `use` (`api/customer_neighbors.rs: (file):1`, `bin/personalization_engine.rs: (file):1`, `sync_api.rs: (file):1`).

## Lỗi theo mức nặng

### Nặng

1. **Lời gọi từ crate binary `src/main.rs` bị bỏ im lặng.** `main.rs` có `mod api; mod cache; mod config; mod connectors; mod customer_store; mod models; mod sync;` trong khi `lib.rs` cũng `pub mod` cùng các file đó, nên mỗi file thuộc 2 crate. Chỗ gọi trong `main.rs` trỏ vào bản của crate binary; driver chỉ hỏi `references` trên bản của crate lib.
   - Bằng chứng: `router` 0 (thật: `main.rs:25`); `spawn_refresher` 0 (`main.rs:22`); `AppState::new` 0 (`main.rs:21`); `Config::load` thiếu `main.rs:18`; `spawn_worker` thiếu `main.rs:24`; `start_refreshing` (`main.rs:23`) cùng loại. Hệ quả: mọi cây ảnh hưởng của API dừng ở `router`, `main` không bao giờ là điểm vào.
   - Tái hiện tí hon: `total_impl` 0 người gọi dù `main.rs:6` gọi qua `mod shapes;`; còn `Inner::ping` tìm được `main.rs:8` vì gọi qua đường dẫn lib `tiny::wrap::Inner`.
   - Không có cảnh báo nào. Hướng sửa gợi ý: hỏi `references` cho mọi crate chứa file (hoặc gộp kết quả theo vị trí định nghĩa), hay ít nhất cảnh báo khi file thuộc nhiều crate.
2. **Field và method cùng tên bị gộp, kết quả là field mà vẫn ghi `exact`.** `AppState { pub pg: …, pub redis: … }` + `impl AppState { pub fn pg(&self) … pub fn redis(&self) … }` (`connectors/mod.rs:16,18,28,31`).
   - `callers 'src/connectors/mod.rs#AppState::pg'` resolve sang dòng 16 (field), trả `api/mod.rs:503`, `connectors/mod.rs:29`, `:122` và bỏ hết 13 chỗ gọi `.pg()`. `callers src/connectors/mod.rs:29` (trong thân method) in đầu là `AppState::pg …:28` nhưng danh sách vẫn là của field. `AppState::redis`: bỏ 10 chỗ gọi `.redis()`.
   - Getter cùng tên field là mẫu rất phổ biến trong Rust, nên lỗi này sẽ gặp thường xuyên.

### Vừa

3. **Hàm/method sinh bởi `macro_rules!` không hỏi được.** `columns.rs#ProfileValues::fill` → `Not found`; `callers columns.rs:219` → `profile_columns` (macro), người gọi là `(file)`; hỏi tên trần `ProfileValues::fill` → 0 người gọi, im lặng (thật: `gate.rs:157`; `from_row` ← `values.rs:31`; `from_postgres` ← `migrate.rs:380`; `lacking_from` ← `pii_writer.rs:296`, `migrate.rs:382`). Tí hon: tên trần `get_one` resolve sang **`src/gen.rs#(file)`**, 0 người gọi, không báo sai symbol.
4. **Gọi trong thân macro dừng ở tên macro.** `non_empty_object` ← `read_column`/`read_json`/`fill_customer` (`unresolved`), `--depth 3` không đi tiếp và không ghi "Some branches were cut". Macro gọi trong macro: `callers columns.rs#fill_customer` = 0 dù được gọi ở `columns.rs:220` trong `profile_columns!`.
5. **Gọi ngầm bị bỏ im lặng:** `From::from` qua `?` (`tiny/src/errors.rs:18`) và `.into()` (`:22`); `Deref::deref` (`wrap.rs:21,25`); `Display::fmt` qua `format!`. Kết quả "0 người gọi" mà không ghi chú gì. README nói "thà thừa còn hơn sót"; với impl của trait chuẩn (`From`, `Deref`, `Display`, `Drop`, `Index`…) ít nhất nên ghi một dòng "gọi ngầm qua toán tử/`?`, không liệt kê được".
6. **Sửa file lớn thì chậm:** thêm chỗ gọi trong `api/customers.rs` (1.427 dòng): hỏi ngay 4,7 s, sau 1 s vẫn 4,3 s; thêm file mới 10,1 s; thêm `impl` 5,8 s. Plan ghi R1 = 390 ms ngay / 8 ms sau 1 s; vòng này không đạt. Kết quả vẫn đúng.

### Nhẹ

7. Dòng `use …` / `pub use …` thành "Direct callers" `(file) [reference]` và được đếm vào `exact` (`pii`: 49 thay vì 37 chỗ gọi thật, 12 dòng `use`). Ở tầng trên chúng thành node `(file):1`. Một dòng `use` không chạy gì; nên tách khỏi tổng hoặc bỏ.
8. Nhiễu `unresolved` theo tên từ thân macro và code bị `cfg` tắt: `Command::new` (`api/mod.rs:532`, trong `#[cfg(target_os="macos")]`), `Map::new()` (`columns.rs:153`), `Vec::new()` (`:195`) thành người gọi của `AppState::new`; `other.keys().all(..)` (`columns.rs:140`) thành người gọi của `WebEventType::all`/`RfmSegment::all`. Lời gọi có tiền tố kiểu khác (`Vec::`, `Map::`) hoặc cú pháp method với hàm không có `self` có thể loại tĩnh.
9. Dòng symbol là đầu doc comment, không phải dòng `fn` (`start` `:231` vs `fn` `:237`, `run` `:292` vs `:293`, `profile_changes_to_value` `:301` vs `:305`).
10. Truy vấn đầu cho một symbol trong lúc rút dữ kiện nền còn chạy mất 0,5–2,3 s (`t02` 2,3 s, `ProfileValues::fill` tên trần 1,2 s).

## Phần chạy đúng

- Trait: `dyn Trait` (`&dyn ClickHouseReader`), generic `T: Shape`, `impl Shape`, `Box<dyn Shape>`, method mặc định gọi `self.area()`, `impl Trait for Vec<T>`: đủ, nhãn `exact` cho method của trait, `likely` cho từng impl, loại đúng chỗ gọi có kiểu cụ thể khác.
- `Self::` qua nhiều khối `impl Customer` ở nhiều file, `self.` trong impl, hàm trùng tên (`find` ×4, `as_str` ×8, `default`, `all`, `bad_request` ×5 → báo "matches several symbols" rõ ràng).
- Closure: `map(|s| s.as_str())`, closure lồng trong `tokio::spawn(async move { loop {..} })`, closure truyền vào hàm generic `FnOnce`, closure cất trong biến rồi gọi, fn pointer cất trong field struct.
- Giá trị hàm: `filter_map(transform::web)`, `map(SegmentListItem::from)`, `map(Shape::area)` → `[reference]`.
- Re-export và re-export đổi tên (`start as start_migration`) lần ra đúng chỗ gọi ở `api/mod.rs:380`.
- `#[cfg(test)]`: ẩn mặc định, có dòng "Hidden: N", `--tests` hiện đúng tên `tests::…`.
- `async`/`.await`, `#[async_trait]`: không ảnh hưởng.
- Sửa code: 7/7 lần giống hệt daemon mới, cả hỏi ngay lẫn sau 1 s; dời dòng giữ nguyên dấu vân tay; đổi tên thì tên cũ báo `Not found`; file mới được nhận.
- Không treo, không lỗi tiến trình; không đụng daemon của repo khác.
