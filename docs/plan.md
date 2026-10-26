# scout (trước đây codetrace) — tra "ai gọi tới đây" để định vị lỗi và đo vùng ảnh hưởng

> Plan này và các báo cáo test (`docs/reports/`) chuyển từ workspace easy-ai-dev (`plans/261025-codetrace/`) sang đây ngày 26/10/2026; từ nay cập nhật tại `docs/plan.md`.
>
> Đổi tên thành **scout** (26/10/2026). Repo `~/www/scout`, GitHub public https://github.com/locdev-2001/scout (`main`). `./install.sh` cài từ đầu; quản lý bằng `scout setup|uninstall|status|stop|logs|update`; MCP tên `scout`, biến môi trường `SCOUT_*`, cache `~/.cache/scout`. Phần dưới vẫn gọi `codetrace` theo lịch sử. Chạy một lần trên máy sạch của GitHub (macOS arm64 + Ubuntu, 26/10, rồi bỏ CI theo ý người dùng): `install.sh` build và `setup` qua trên cả hai; driver PHP chết vì máy không có `rg`. Đã sửa: không có `rg` thì liệt kê bằng `git ls-files` và tự tìm trong file (backend: cùng kết quả, câu đầu ~9 s thay vì ~3 s); `rg` thêm `--hidden` vì trước đó bỏ sót lời gọi thật trong thư mục ẩn (`.claude/skills/.../sandbox-eval.php` → `Chatbot::getResponse`). Các ngôn ngữ khác trên macOS chưa được chạy tới (bước bị dừng ở PHP).

Trạng thái: đang làm bản đầu (chưa commit, thử ở local trước).

## Mục tiêu

AI hỏi được hai câu, không cần index, kết quả luôn theo code hiện tại:

- `locate <file:line>`: dòng này thuộc symbol nào (đi từ stack trace).
- `callers <symbol|file:line>`: mọi nơi gọi tới, đi ngược nhiều tầng tới route, event, job, command.

## Giá trị cốt lõi (không thoả hiệp)

1. **Tốc độ:** mỗi lần tra dưới 100 ms, ở mọi repo.
2. **Chính xác tuyệt đối:** không giới hạn tầng/số node; không sót caller nào; mỗi kết quả `exact` là thật. Chỗ gọi động mà không công cụ tĩnh nào quyết định được (`$obj->$name()`) phải được liệt kê rõ, không được bỏ im lặng.
3. **Tương thích mọi ngôn ngữ và framework:** mỗi driver dùng chính engine của ngôn ngữ/framework đó; không ghi cứng phiên bản. Lúc khởi động, driver tự dò hành vi thật của engine (ví dụ: fork + opcache có thấy file vừa sửa không) rồi chọn chế độ nhanh, hoặc chế độ chậm nhưng đúng và báo rõ.

4. **Tối ưu tài nguyên máy:** lúc rảnh không tốn CPU (theo dõi file bằng inotify, không quét định kỳ); RAM chỉ giữ dữ kiện cần cho truy vấn và có số đo cụ thể; không ghi index ra đĩa; daemon tự tắt khi rảnh; bộ nhớ của engine (ví dụ opcache) đặt theo nhu cầu đo được, không đặt dư. Lúc đang tra cũng phải nhẹ: một truy vấn chỉ làm phần việc của file đã đổi (dữ kiện kiểu lấy từ Reflection được giữ theo dấu vân tay của file, chỉ fork khi có file đổi), mỗi kết quả báo kèm CPU-ms và RAM đỉnh của truy vấn đó.

5. **Mọi hệ điều hành** (Linux, macOS, Windows) và **mọi agent** (CLI + MCP).
6. **Thiết lập một lệnh**, và output gọn để giữ context của agent nhỏ.

## Lộ trình tới mục tiêu cuối

1. S4/S5 dưới 100 ms: xong (tra ngay sau khi lưu 25–95 ms, cả 6 tình huống khớp daemon mới).
2. MCP server trong cùng binary + output gọn cho agent: xong (`codetrace mcp`, `--compact` gom theo file).
3. Driver chung cho ngôn ngữ chưa có driver riêng: xong (`driver-generic`, mọi chỗ gọi theo tên, gắn `unresolved`).
4. Driver TS/JS/Vue: bản đầu xong, xem ghi chú bên dưới.
5. Tương thích HĐH: core có lớp `src/ipc.rs` (Unix socket; Windows dùng TCP loopback + token trong thư mục tạm của người dùng, tách tiến trình bằng `DETACHED_PROCESS`), dấu file theo từng HĐH (Windows: giờ ghi, giờ tạo, kích thước), tài nguyên ngoài Linux báo "không đo". `cargo check` sạch cho `x86_64-pc-windows-gnu` và `aarch64-apple-darwin`; chưa chạy thật trên hai HĐH đó. PHP không có pcntl (Windows): driver làm việc ngay trong tiến trình của nó và tự khởi động lại khi file nó đã nạp bị sửa; ép thử trên Linux bằng `CODETRACE_NO_FORK=1`.
6. Lệnh thiết lập: `install.sh` đã viết, chưa chạy (nó sửa cấu hình agent của người dùng, cần hỏi trước).

### Ghi chú driver JS/TS/Vue

Dựng nguyên chương trình TypeScript (LanguageService) cho JS/Vue của backend: liệt kê + tách script 1.501 file Vue 2,3 s; dựng chương trình 8,4 s (4.775 file kể cả thư viện); heap 1,4 GB. Quá nặng. Hướng chọn: chỉ dùng parser của TypeScript và `vue/compiler-sfc` theo từng file, giữ dữ kiện theo file như driver PHP.

Cách driver hiểu code (`drivers/javascript/driver.mjs`):

- Engine lấy từ chính repo: `typescript`, `vue/compiler-sfc`, và Vite. Đường dẫn module (alias), file `input`, và tên component tự import đều hỏi Vite (`loadConfigFromFile`, `resolveConfig`, `api.findComponent` của plugin tự import) trong một tiến trình con chạy một lần. Việc này chạy `vite.config.*` và plugin của repo, giống khi chạy dev server. Không có Vite thì dùng `paths` của tsconfig/jsconfig.
- Symbol là `file#key`: hàm, method của class (`Lop.method`), hàm lồng (`useX.load`), biến cấp module, `default` (component), `(module)` (code cấp module).
- `exact`: import có tên, import mặc định, namespace, re-export (kể cả `export *`), `export default` một biến; `this.m()` trong class/component; template (biểu thức, `@click`, tag component, `v-for`/slot đè tên được loại); JSX; hook của Vue; hàm trong object truyền cho code khác (nối về hàm đã truyền); file nạp qua `import()`, `require()`, `import.meta.glob`.
- `likely`: `this.m()` viết ở class cha chạy bản override của class con.
- `unresolved` (liệt kê, không bỏ): gọi member qua đối tượng không rõ nguồn khi đích là method/member, hàm được `defineExpose`, export CommonJS, hoặc module bị nạp động; tên chưa có nguồn khi đích là hàm được export (có thể được plugin tự import); tag chưa có nguồn trùng tên file component.
- Điểm vào: file `input` của Vite, và từng trang nạp qua `import.meta.glob` từ file đó.
- Rút dữ kiện lần đầu bằng 2 tiến trình con ưu tiên thấp; xong thì thoát, RAM trả lại hệ thống. Driver giữ dữ kiện gọn (bảng chuỗi + mảng số).

Nghiệm thu trên repo mẫu (`scratchpad/js-fixture`, mỗi kiểu nối một ví dụ, so với danh sách liệt kê tay): 19/19 đích ra đúng người gọi, không thừa, không sót.

Số đo trên bản sao backend (2.796 file JS/TS/Vue), hàm tiện ích dùng rộng (3.732 symbol bị ảnh hưởng, 14.971 dòng `unresolved`):

| Tình huống | Thời gian | Khớp daemon mới |
| --- | --- | --- |
| Rút dữ kiện lần đầu (2 tiến trình con) | ~4,6 s, đỉnh ~600 MB, sau đó driver giữ ~180 MB | |
| Hỏi lại, code không đổi | 63–75 ms (trước khi core giữ câu trả lời: 200–300 ms) | có |
| J1: thêm hàm gọi tới đích ngay trong file đích | 75 ms | có |
| J2: chèn một dòng vào component giữa cây | 100 ms | có |
| PHP S1–S6 chạy lại sau thay đổi ở core | 20–82 ms | cả 6 |

Hai thay đổi ở core để đạt mức này: core giữ câu trả lời `callers` của driver khai báo `"cache": ["callers"]` và chỉ hỏi lại các symbol mà `sync` của driver liệt kê trong `invalidated` (driver JS ghi mỗi câu trả lời đã đọc chỉ mục và file nào); câu trả lời được chia sẻ (`Arc`), không chép, và request được ghi qua bộ đệm. Bản gọn (`--compact`) của truy vấn trên còn ~160 KB vì `unresolved` được gom theo đích kèm lệnh liệt kê, thay vì 6 MB của cây đầy đủ.

Còn thiếu: nối trang Inertia với `Inertia::render('ten-trang')` bên PHP (hiện trang là điểm vào); `super.m()` đang là `unresolved` dù có thể quyết định tĩnh; thêm/xoá file `.vue` phải hỏi lại plugin (~0,5 s, là trường hợp duy nhất trên 100 ms).

### Vòng test 1 (3 phiên con mới, 25/10) và các bản sửa

Báo cáo: `reports/test-php-1935.md`, `reports/test-js-1943.md`, `reports/test-flow-1933.md`. Người gọi trực tiếp đúng ở mọi đích dễ (PHP 4/4 + 4 phụ, JS 8/8, 57/57 tag, `locate` 13/13). Lỗi tìm ra và cách sửa:

| Lỗi | Sửa | Kiểm lại |
| --- | --- | --- |
| Treo 30 phút qua MCP | chờ driver có kiểm còn sống (driver chết mà pipe còn mở vẫn được phát hiện), khoá file một daemon mỗi repo, watchdog không tắt khi đang trả lời, log ghi nối, reap tiến trình con, dòng `Waited:` cho thời gian chờ | 5 client cùng lúc → 1 daemon, đủ câu trả lời; driver giả chết giữ pipe → trả lời sau 1 s |
| PHP `->forceFill()->save()` mất kiểu → `likely` cho mọi override (10 chỗ gọi → 238 điểm vào) | `@return $this/static/self` là class được gọi trên, không phải class khai báo | `ChatMessage::save` trực tiếp 140 → 34, `likely` 111 → 3; `assignMember` 1.444 → 382 symbol, bản gọn 130 → 33 KB |
| `parent::save()` coi là gọi chính nó | `parent::`/`self::`/`X::` gọi cố định, chỉ `static::` đa hình | cạnh sai biến mất |
| `return (new X)->m()` không suy kiểu | từ khoá trước `(` không phải tên hàm | `unresolved` sai ở `WorkflowNode.php:1874` biến mất |
| JS bỏ sót hàm destructure từ composable, Pinia `unresolved`, composable thành "người gọi" `exact` | lần theo kết quả hàm (`const {a: b} = useX()`, `const s = useX(); s.m()`, `useX().m()`), chỗ `return {...}`/`defineStore` thành `unresolved` | repo mẫu: đúng dòng gọi, `exact`/`likely` |
| JS bỏ sót `{ mapResponse: fn }` rồi `x.mapResponse()` | tên thuộc tính thành tên tra thêm | `unresolved` đúng chỗ gọi |
| Nhiễu theo tên (`path.resolve`, `Promise.resolve`, bundle minified) | `require` gói ngoài = gói, bỏ member của đối tượng runtime, bỏ file bundle, không tra tên member cho object giao thư viện | `unresolved` 14.971 → 3.062 trên hàm dùng rộng |
| Nhóm file rỗng, thiếu số dòng, MCP thiếu `depth`, cảnh báo PHP ở `locate`, RAM đếm trùng fork | symbol cầu nối có file/dòng, bản gọn có `tên:dòng`, MCP `depth`, cảnh báo chỉ ở `callers`, RAM theo PSS | |

Hồi quy sau khi chép sang bản chính: PHP S1–S6 giống hệt daemon mới (20–74 ms), JS J1/J2 giống hệt (55/90 ms, hỏi lại 58–66 ms), chế độ không fork S1–S6 giống hệt (chậm 0,7–30 s sau mỗi sửa).

### Ngôn ngữ thêm (25/10)

| Ngôn ngữ | Engine | Kiểm |
| --- | --- | --- |
| Rust | `rust-analyzer` qua driver LSP chung (`src/lsp.rs`, mỗi ngôn ngữ một `driver.json` có khối `lsp`) | repo mẫu đúng hết (gọi thẳng, giá trị hàm, macro `$crate::`, trait `dyn`/generic là `likely`, method trùng tên bị loại); 9 kiểu sửa (chú thích, đổi kiểu trong thân hàm, thêm chỗ gọi, thêm `impl`, thêm hàm có `use … as`, xoá hàm, đổi chữ ký) đều giống hệt driver mới. Engine thật (165 file, 1.670 hàm): rút dữ kiện ở nền xong sau 51 s, sau đó hàm dùng rộng 126 chỗ gọi 4 ms; R1 thêm chỗ gọi 390 ms nếu hỏi ngay, 8 ms nếu hỏi sau 1 s; R2 chèn dòng 3–4 ms; RAM ~3,3 GB (của rust-analyzer). Cách đạt: đọc trước mọi file (`semanticTokens`), rút dữ kiện ở nền, `resolve` qua chỉ mục tên, sửa trong thân hàm thì dời dòng thay vì hỏi lại, chèn/xoá nguyên khai báo chỉ ảnh hưởng tên viết trong đó, tính lại khi rảnh ngay sau sync |
| Python | `ast` của CPython (`drivers/python/driver.py`) | repo mẫu đúng; `grouped_bar` 62/62 khớp `rg` (sau khi thêm `from x import *` và thư mục script là gốc import) |
| React (TSX/JSX) | driver JS | yona `Button` 106/106 khớp `rg` |
| Svelte 5 | `svelte/compiler` trong driver JS | widget `Button.svelte` 29/29 khớp `rg`; hàm `.svelte.ts` gọi từ template đúng |

Lỗi của rust-analyzer gặp khi làm: `references` hỏi ngay sau khi đổi tài liệu trả thiếu (3/3 lần); sửa bằng cách chờ nó báo hết bận (`experimental/serverStatus`), tắt bộ theo dõi file riêng của nó (`files.watcher: client`), mọi thay đổi đi qua văn bản tài liệu.

C/C++, Java, C#: người dùng chọn tạm chưa cài engine; đi qua driver chung (liệt kê theo tên, `unresolved`).

### Vòng test 2 (phiên con mới, khó hơn, 25/10)

Báo cáo: `reports/test2-python-2047.md`, `reports/test2-js-2051.md`, `reports/test2-php-2041.md`.

Python và JS đã sửa, đã chép sang bản chính (bản cũ giữ ở scratchpad `ct-base` để so):

| Chỗ sót / nhiễu | Sửa | Kiểm lại |
| --- | --- | --- |
| Python: `super()`, `__init__` qua `Cls(...)`, import trong thân hàm, chuỗi `pkg.core.helper`, `try/except ImportError`, thư mục script ưu tiên | phạm vi và import viết lại theo `Scope`; kế thừa qua `ancestors_of`/`subclasses_of` | repo mẫu đủ; `grouped_bar` 62/62, 177 ms |
| JS: hàm bọc `useCallback`, `memo`, `lazy`, `obj.prop = fn` lồng trong hàm | tên theo biến bọc, cạnh `lazy import`, `debounce(...).cancel()` nối qua kết quả hàm | `debounced.cancel` 0 → 14 `exact` + 9 `unresolved` (đủ 5 dòng báo cáo); `snooze`, `markAllRead` đủ |
| JS: hàm lấy ra bằng destructure rồi truyền đi (`onCreated={reload}`, `@click="toggleSidebar"`) | ghi theo tên thuộc tính; prop truyền vào thẻ component chỉ tra trong file của component đó, thẻ DOM thì không tra | `reload` ← đúng `NewWebHook.tsx:31`, không kéo `onCreated()` của 5 component khác; `toggleSidebar` 102 `exact` sai → 3 dòng đúng |
| JS: receiver có kiểu `Recorder \| undefined`, snippet Svelte | bỏ `undefined`/`null` khỏi kiểu hợp; snippet chỉ khớp lời gọi | `rec.start()` thành `exact`; snippet hết nhiễu `card.actions` |
| JS: bundle đã format lại (`public/vendor/*/app.js`, `ckeditor.js`) | nhận theo tỉ lệ tên một chữ cái trong tham số/biến (≥ 100 tên, > 70%; bundle 94–100%, mã thật ≤ 29%) | hết nhiễu bundle; khởi động lạnh backend 10,5 s → 6,6 s |
| JS: dòng tự gán `x.m = ...` thành người gọi | vế trái phép gán không phải chỗ dùng | |

PHP (10 nhóm lỗi trong báo cáo), đã sửa, kiểm trên đúng dòng bằng chứng của báo cáo:

| Lỗi | Sửa | Kiểm lại |
| --- | --- | --- |
| Daemon treo vô hạn (driver nhận câu hỏi rồi im, CPU 0%) | core đo CPU cả cây tiến trình driver khi chờ: 20 s không trả lời mà cũng không làm gì thì ghi log câu hỏi bị nuốt, giết cả cây, khởi động lại, hỏi lại một lần, rồi cảnh báo thay vì treo | driver giả nuốt câu hỏi: trả lời sau 40 s kèm cảnh báo; log ghi đúng câu hỏi. Nguyên nhân gốc chưa tái hiện được (4 lần thử), log sẽ chỉ ra khi gặp lại |
| Trait: 0 người gọi | trait vào danh sách tổ tiên; so method theo nơi viết (`Trait::m`) thay vì class khai báo theo Reflection; `self::` trong trait là class dùng nó | `sanitizeAiResponse` 0 → 7/7, `stripMarkup` 0 → 5/5, `Chatbot::staticSanitizeAiResponse` ← `:59`, `HydratesFromCustomerStorage::refresh` ← `ChatUserObserver.php:67` `exact` |
| `$x = $c ? $o->m() : null; $x->n()` mất kiểu | kiểu phép gán = hợp các nhánh `? :`, `?:`, `??` (nhánh `null` bỏ qua) | repo tí hon đúng hết; `pushInventoryOut`, `cancelInvoice`, `cancelOrder`, `parseWebhook`, `applyWebhookEvent` ra đủ dòng của báo cáo |
| `$class::m()`, `$this->$m()` bị bỏ qua | thành `unresolved` (gọi động: cho mọi method cùng họ class; `$x::m()` cho mọi `m`); `$c = X::class; $c::m()` ra X | `checkStoreStock` ← `BotFunction.php:160`; `getPromotionAttribute` ← `:160`, `BaseBotFunction.php:818`; `forBot` ← 4 factory |
| Anonymous class: tên sai, trùng, 0 người gọi | scope riêng `Outer@anonymous_<dòng>`, class cha đọc từ mã nguồn; người gọi = người gọi method cùng tên ở class cha/interface (`likely`) | `fromRaw` ← `Inbound.php:3148`; Nhanh/Shopify khác tên |
| `Artisan::call('x')`, `$this->call('x')`, `Schedule::command('x')` | Laravel tìm chuỗi tên lệnh làm đối số của các hàm chạy lệnh | `datasource:retrain` ← `DataSourceController.php:709`; `datasources:rerank` ← 2 controller; `email:sync` ← `routes/console.php` |
| `app(X::class)()` | gọi một đối tượng như hàm = `__invoke` | `DisconnectPeDatabase::__invoke` ← `AppServiceProvider.php:376,377` |
| Route closure, scheduler mất entry point | closure ở cấp file thành scope `routes/web.php:<dòng>`, Laravel nối route closure vào đó; code cấp file không ai gọi tới = entry point | `shouldResumeOnboarding` có `route GET / (bots.index)`; `CleanupExpiredPresence::handle` có `routes/console.php` |
| `Closure::fromCallable([$this,'m'])` trong constructor kéo 687 symbol | callable cất vào thuộc tính là `unresolved`, không đi tiếp | `toShortLink`: 1 dòng `stored callable` |
| Symbol cầu nối `Job::dispatch*` ăn một tầng `--depth` và phình output | tầng cầu nối không tính độ sâu, không liệt kê ở "Further up"; tên trait không còn dùng làm gợi ý receiver | job `--depth 1` ra đủ nơi dispatch, 0 `unresolved` thừa; đích 4a 9,3 → 6,3 KB |
| Cảnh báo file test hỏng ở mọi câu trả lời; dòng trong `tests/` lộ ra | thư mục test lấy từ `autoload-dev`; cảnh báo về file test chỉ hiện với `--tests` | file đó dùng trait không tồn tại (lỗi thật của repo) |

Còn lại, chưa sửa: `app(X::class)->m()` vẫn `likely` dù X là class cụ thể; RAM driver PHP 600–1.000 MB (PSS) khi ấm; warm-up PHP lần đầu ~30 s trên backend.

### Vòng test 3 (phiên con mới, 26/10)

Báo cáo: `reports/test2-rust-2210.md`, `reports/test3-python-2215.md`, `reports/test3-php-2224.md`. Các bản sửa vòng 2 đều giữ đúng; sửa code trong lúc hỏi khớp daemon mới ở mọi lần thử (Rust 7/7, Python 20/20, PHP 7/7); 4 client song song không treo.

| Lỗi | Sửa | Kiểm lại |
| --- | --- | --- |
| Rust: lời gọi trong `src/main.rs` (crate khác khai báo lại `mod`) bị sót | lời gọi viết trong file của crate khác (`main.rs`, `src/bin/`, `examples/`, `benches/`) được hỏi `definition` | `router`, `spawn_refresher` ← `main.rs`; repo tí hon `total_impl` ← `main.rs:6` |
| Rust: field và method trùng tên gộp làm một | ưu tiên symbol gọi được | `AppState::pg` 13/13 lời gọi `.pg()`, không lẫn field |
| Rust: dòng `use` tính là người gọi; `Command::new` gắn vào `AppState::new`; dòng symbol ở doc comment | bỏ dòng `use`; đường dẫn ghi rõ kiểu khác thì không phải; dòng = dòng tên | `pii` 49 → 37 |
| Python: thêm/xoá file đọc lại cả repo (~60 s) | chỉ đọc lại file có chữ trùng tên module vừa thêm/xoá | |
| Python: khởi động lạnh 53 s (8.419 file), method nhiều lớp con 0,7–1,25 s mỗi lần | đọc trước bằng vài tiến trình con mức ưu tiên thấp; bỏ gắn `parent` cho mọi node; chỉ mục lớp con; giữ câu trả lời tới khi có file đổi | lạnh 53 → 11,6 s; ấm 19 ms; `send` (nhiều lớp con) 39 ms; câu rất rộng 212 s/46,8 MB → 52 s/0,89 MB |
| Python: định nghĩa trùng tên (`@overload`, setter) làm `locate` sai | giữ mọi khoảng định nghĩa | `delegation_context.py:182` đúng hàm |
| Python: `Cls.static()` thành lời gọi `__init__`; MRO sai | dùng class qua thuộc tính là tham chiếu; MRO theo C3; `super()` hợp tác trong đa kế thừa là `likely`; lớp con không có `__init__` riêng | `BasePlatformAdapter.__init__` 55 → 33 dòng đúng; `D1.greet` → `A1.hello`; `Right.setup` ← `Left.setup` |
| Python: `from . import repo as storage`, `getattr(o, "m")()`, `super(Mixin, self)`, `__post_init__`, decorator dạng gọi thành entry, hàm lồng bị tra theo tên member | sửa từng chỗ | fx3 đúng |
| Câu hỏi lớn chặn cả repo kể cả khi client đã bỏ | giữa các tầng, daemon kiểm client còn chờ không | câu kế tiếp chờ 2,8 s thay vì ~88 s |
| PHP: `Model::query()->find()` rồi gọi method bị bỏ âm thầm | kiểu không trỏ tới class có thật (template `TModel`) coi như chưa biết → `unresolved` | `SyncRunJob.php:43,59`, `AutoReplyController.php:19` có mặt |
| PHP: route arrow fn mất entry; closure `group` thành entry giả | arrow fn cấp file thành scope; chỉ code cấp file (không phải closure) mới là entry | `onboardingEntryRoute` có `route GET /connect/facebook` |
| PHP: scope, accessor ra 0; constructor thừa kế; `[$this->prop, 'm']` | Laravel nối `scopeX` ↔ `->x()`, `getXAttribute` ↔ `->x` (file có nhắc tên model); `new Con` chạy constructor thừa kế | `Campaign::scopeActive` 1 `likely` + 4 `unresolved`; `new SapoProvider` có mặt |
| Tên không tồn tại vẫn rc=0 và ra dòng trong `.md/.json/.map` | tên có namespace mà không driver nào nhận là `Not found`; driver chung bỏ file văn bản/dữ liệu | |
| Driver Laravel còn kẹt (watchdog cắt sau 20 s) | log ghi thêm trạng thái từng tiến trình trong cây | nguyên nhân chưa tái hiện được (boot riêng 3/3 lần ~200 ms) |

Sau vòng 3 (cùng ngày, tiếp):

| Việc | Kết quả |
| --- | --- |
| PHP hỏi sau khi sửa | Laravel không còn xoá toàn bộ cache khi một file PHP đổi: mỗi kết quả quét nhớ từ cần tìm và file đã khớp, chỉ bỏ cái bị ảnh hưởng (kèm sửa `PhpFileCache` giữ nội dung cũ mãi). Quét kiểm độ mới dùng đủ luồng: 10–19 ms → ~5 ms. 6 kịch bản, hỏi sau 1 s: 75–179 ms → **19–42 ms**, đều khớp server mới. Hỏi ngay trong cùng tích tắc lưu file: 24–317 ms (đang chờ bản đọc lại xong) |
| Laravel | middleware (75 route gom một dòng; global = "every HTTP request"), form request ← action, facade (`Repository::remember` ← 51/51 dòng `Cache::remember`), policy ← `authorize('update', ...)` |
| PHP `__call`/`__callStatic` | lời gọi method không khai báo trên class có `__call` là `likely` |
| Python | `with`/`for`/`x()` → `__enter__`/`__exit__`/`__iter__`/`__call__`; biến cục bộ gán một lần từ `X()` có kiểu (`cart = Cart(); cart.add()` thành `exact`); `cls()` trong classmethod |
| Rust | hàm do `macro_rules!` sinh (`columns.rs#fill@219` ← `gate.rs:157`); `?`/`.into()` cho `impl From` (`unresolved`, trong hàm có chữ ký nhắc kiểu đích); 9/9 kiểu sửa trên fixture vẫn khớp daemon mới |
| Kết quả lạnh khác bản tham chiếu cũ 96 dòng | đã soi: dòng đăng ký route gắn với closure `group`, dòng helper test được ẩn, thêm `$jobClass::dispatch` và `Schedule::command` — đều đúng |
| Agent | MCP qua stdio (`initialize`, `tools/list`, `callers`, `locate`) chạy đúng; Claude Code báo Connected, Codex liệt kê `codetrace` enabled |
| Hệ điều hành | `cargo check` sạch cho `x86_64-pc-windows-gnu` và `aarch64-apple-darwin`; chưa chạy thật (máy không có wine). 26/10: không có Mac. Rà code macOS: dọn cây tiến trình đọc `/proc` (macOS không có) nên `stop` chỉ giết tiến trình gốc, sót rust-analyzer/worker; đã thêm đường `pgrep -P` (so trên Linux: cùng danh sách con). Python `spawn` (mặc định macOS) ép thử trên Linux: hermes 393 caller giống `fork`. Còn khác trên macOS: RAM/CPU mỗi câu báo "không đo", watchdog treo chỉ theo 300 s |

Sự cố: lúc 23:05 kernel hết RAM và giết một tiến trình `claude-desktop` khi hai rust-analyzer chạy cùng daemon backend. Từ nay mỗi lần chỉ một rust-analyzer, tắt daemon scratchpad ngay sau khi test.

Tiếp theo (cùng ngày):

| Việc | Kết quả |
| --- | --- |
| PHP hỏi ngay khi vừa lưu file | dữ kiện chỉ dời số dòng thì dời dòng trong các câu đã cache thay vì tính lại; vị trí không còn nằm trong dấu vân tay class. 6 kịch bản hỏi ngay: S1–S4, S6 **25–44 ms** (S2 317 → 39 ms); S5 (đổi kiểu trả về, 218 chỗ gọi đổi) 181 ms khi warm-up đã xong (sync 104 ms cập nhật 218 chỗ gọi + tính lại câu bị ảnh hưởng; 267 ms nếu warm-up còn chạy), hỏi sau 1 s 42 ms. Chạy lại trên bản chính cuối: S1–S4, S6 21–43 ms, đều khớp server mới |
| Rust hỏi ngay khi vừa lưu file lớn (1.427 dòng) | câu cũ được tính lại từng câu một lúc rảnh thay vì cả loạt chặn câu hỏi: 4,7–10 s → 0,5–1,1 s ngay, 22–127 ms sau 1 s |
| Laravel macro | closure đăng ký `Builder::macro('x', ...)` thành method `Builder::x`; `ContentQuery::exclude` ← macro `excludingCustomerStorage` ← 7/7 chỗ gọi (dòng thứ 8 của `rg` nằm trong docblock) |
| Python | `len(x)` & các builtin gọi dunder; `__getattr__` ← đọc thuộc tính không có (thuộc tính gán qua `self.x =` và trong thân class được ghi nhận) |
| Rust | `Display::fmt` (`{}`, `.to_string()` trừ trên literal), `Debug::fmt` (`{:?}`, `dbg!`), `Iterator`/`IntoIterator` (`for … in`), trong hàm nhắc tới kiểu đích |
| Hồi quy | Rust fixture 9/9, JS fixture chỉ `legacy.cjs`, Python fixture 44/44 giống lần trước |

Agent dùng thật: gọi tool `callers` qua server MCP mà phiên Claude Code đã mở từ trước thì lỗi "cannot start server: No such file or directory". Nguyên nhân: binary được build lại trong lúc server MCP sống, Linux đổi tên file cũ thành `codetrace (deleted)`, server không khởi động được daemon và tính sai tên socket. Sửa: dùng đường dẫn binary mới khi file cũ đã bị thay; driver thiếu engine (không có `node`, `php`) chỉ bị bỏ qua kèm cảnh báo thay vì làm daemon không lên. Kiểm: mở server MCP, thay file binary, gọi `callers` → trả lời đúng. Server MCP đang mở trước bản sửa phải kết nối lại một lần.

Rust `Deref`/`DerefMut`: `*x` một ngôi (không phải phép nhân) trong hàm nhắc kiểu đích, `unresolved`; repo tí hon đúng, fixture 9/9 vẫn khớp.

S5 chậm vì `subclasses()` duyệt cả ~4.700 class mỗi lần (135/192 ms); thay bằng chỉ mục class cha → class con: 6 kịch bản hỏi ngay khi vừa lưu file **26–67 ms**, cả 6 khớp server mới.

Chốt chặn trong driver PHP/Laravel: tiến trình con fork ra mà 10 s không trả lời và không dùng CPU thì bị dừng, chỉ câu hỏi đó báo lỗi (driver và cache giữ nguyên); nơi không đọc được CPU dùng giới hạn 300 s. Kiểm: con ngủ bị dừng sau 10,0 s, con bận 15 s vẫn trả lời; 6 kịch bản PHP 24–66 ms, khớp server mới.

Chưa làm: nguyên nhân driver Laravel thỉnh thoảng kẹt (watchdog cắt sau 20 s, log ghi trạng thái từng tiến trình); chạy thật trên Windows/macOS (chỉ biên dịch được kiểm).

### Vòng test 4–5 (phiên con mới, 26/10) và các bản sửa

Báo cáo: `reports/test4-php-0027.md`, `reports/test4-js-0034.md`, `reports/test5-python-0113.md`. JS vòng 4: hơn rg 16, ngang 21, kém 13 đích; các lỗi kém đã sửa:

| Lỗi | Sau sửa (đo thật) |
| --- | --- |
| Monorepo workspace (`@debtnote/money-core`, `@oh-my-pi/pi-utils`) bị bỏ im lặng | đọc `package.json` của từng gói (`exports`/`types`/`main`, `dist/` → `src/`): debtnote 0 → 5/5, oh-my-pi `untilAborted` 0 → 316 dòng khớp rg |
| `./x.js` trỏ tới `x.ts`; repo TS không có `package.json` ở gốc | ánh xạ `.js`→`.ts`, driver JS bật theo file `.ts/.tsx/.vue/.svelte`: claude-code `isNoiseFile` Not found → 1 + 3 test, đúng |
| Số dòng Vue lệch 1–8 dòng | dòng tính từ offset: `locate` FlowCheckout đúng 3/3, `toMoney` đúng dòng |
| oh-my-pi: câu hỏi TS bật rust-analyzer 7 GB, >120 s | rust-analyzer chỉ bật khi câu hỏi cần file `.rs`; TS lần đầu 10 s, ấm 7–13 ms; Rust vẫn đúng (personalization-engine 26 s lần đầu, 38 ms ấm) |
| Nhiễu: hằng cấp module coi mọi importer là người gọi, đọc thuộc tính `.error`/`.global` | `buttonVariants` 31 thừa → 4/4; `actions.error` 15 thừa → 9/9; chỉ ghi phép đọc thuộc tính khi giá trị được chuyển đi |
| Sót: gọi qua template ref, slot scope, method của object factory, `.mdx`, import nội bộ không phân giải | đều được liệt kê (`.mdx` 19/19); import lạc thành `unresolved` thay vì bị bỏ |
| PHP: scope Eloquent viết nhiều dòng; route trong `Route::group` lồng 4 tầng closure | 5 unresolved → 4 `likely`; chuỗi closure cấp file gộp về thẳng file |
| Generic `Class::method` đọc lại file mỗi câu (333 ms) | chỉ mục hash từ của từng file: 17 ms, cùng 22 kết quả |
| Cảnh báo không liên quan (vite.config) ở mọi câu trả lời | cảnh báo của driver chỉ hiện khi câu trả lời có file thuộc ngôn ngữ đó |
| Hook: sửa JSDoc trên hàm báo `(module)`; hỏi ngay sau `stop` bị "Connection reset" | quy về khai báo bên dưới; `stop` chờ tiến trình thoát |

Python vòng 5: 10/13 lỗi vòng 3 đã hết (thêm/xoá file 57 s → 8 ms, `BasePlatformAdapter.*` 0,7–1,25 s → 13–135 ms). Lỗi mới đã sửa:

| Lỗi | Sau sửa |
| --- | --- |
| `package-dir = {"robomp" = "src"}` không hiểu: `issue_key` 0 caller | đọc `package-dir` (pyproject/setup.cfg): 15 exact + 53 test, khớp rg |
| `registry.get_entry()` (instance import), `get_broker().m()` bị bỏ | `ToolRegistry.get_entry` 3 → 10/11 (`likely` cho instance cấp module), `list_sessions` 7/8, `scope_for_session` 3/3 |
| Nhiễu dunder ngầm: `__enter__` 2.072 dòng, `__call__` 332 | chỉ tính ở file import class; nhận biết giá trị tạo ở cấp module và `self.x = Cls()`: `__enter__` 66, `__call__` 1 + 2 exact mới (`self._resample(chunk)`) |
| Thêm file giữa warm-up: 49 s | giữ kết quả warm-up, chỉ đọc lại file nhắc module đổi: 11 s |

Hồi quy: 6 kịch bản sửa PHP đều khớp server mới (24–66 ms).

Rust vòng 5 (`reports/test5-rust-0215.md`): lỗi vòng 2 chính đã hết (`main.rs` khai lại `mod`, field/method trùng tên `pg` 13/13, `pii` 37/37). Lỗi mới đã sửa:

| Lỗi | Sau sửa |
| --- | --- |
| `codetrace stop` khi rust-analyzer đang nạp để lại tiến trình mồ côi ~10 GB (đường dẫn tới OOM) | `stop` giết cả cây; driver-lsp tự thoát và tắt rust-analyzer khi tiến trình cha chết (kiểm: `kill -9` server giữa lúc nạp → 0 tiến trình còn lại) |
| driver-lsp panic với ký tự nhiều byte (`'中'`), mỗi câu Rust ở oh-my-pi khởi động lạnh 20 s | chỉ cắt chuỗi tại `*`; hỏi lại sau 60 s: 82 ms |
| Workspace Cargo không ở gốc git (codex: `codex-rs/`) → không có driver Rust | bật theo file `.rs` ở bất kỳ đâu; codex `map_fs_error` 11/11 exact, tách đúng 2 hàm trùng tên |
| codex: câu đầu 589 s, rust-analyzer 11,4 GB + driver-lsp 4,1 GB | chỉ mục tên → file thay vì mọi lần xuất hiện (driver-lsp 418 MB); không phân tích trước khi >1.500 file; tắt build script, proc-macro, cache priming: câu đầu 17 s, rust-analyzer 4,2 GB (6,3 GB sau câu xuyên crate); personalization-engine 3,27 → 2,38 GB, kết quả giữ nguyên |
| Lỗi từ rust-analyzer (content modified) bị đọc thành "không ai gọi" rồi lưu | hỏi lại tối đa 3 lần; vẫn lỗi thì trả lỗi (không lưu, có cảnh báo); token lỗi không lưu |
| `rust-toolchain.toml` ghim bản chưa cài → rustup có thể tự tải ~1 GB | `RUSTUP_AUTO_INSTALL=0`; lỗi nạp dự án hiện thành cảnh báo |

Đĩa: ổ gốc đầy 100% trong lúc test (máy vốn gần đầy). Thư mục `target` rust-analyzer dựng khi còn chạy build script (codex 947 MB, mỗi repo 0,5–0,75 GB) đã xoá; tắt build script thì không còn tạo ra.

Tiếp (26/10, repo tí hon `scratchpad/rust6` + personalization-engine giữ nguyên `pii` 37, `pg` 13):

| Lỗi | Sau sửa |
| --- | --- |
| Hàm do `macro_rules!` sinh: `file#fill@11` sụp về macro bao ngoài; `make_getter!(get_one, 1)` → Not found | `fill@11` 1/1, `get_one` 2/2 exact (tên trần cũng ra `get_one@17`) |
| Gọi macro `run_work!()` ghi `reference` | `call`; chuỗi `inner_work` ← `run_work` ← `outer` ← `top` |
| Test qua `#[path = "x_tests.rs"]`, `#[test]` ngoài `mod tests`, `#[cfg(test)] mod helpers` không bị ẩn | ẩn cả 3 (theo thuộc tính của item bao quanh và tên file `_tests`/`_test`) |
| `drop(x)` | thành lời gọi ngầm `Drop::drop` (`unresolved`, trong hàm nhắc kiểu đích) như `Display`/`From` |
| Một câu không giới hạn depth chặn mọi câu khác (hermes `get_hermes_home` 72 s) | hỏi từng lô 200 symbol, dừng khi quá 10 s **và** quá 500 nút, nói rõ chỗ cắt: 10,5 s; `locate` chạy cùng lúc chờ ≤7 s (trước >30 s). Bản đầu chỉ xét 10 s đã cắt nhầm câu đầu lúc driver PHP còn warm-up (hồi quy bắt được), nên thêm điều kiện số nút. Hồi quy PHP sau đó: 6/6 khớp server mới, 24–61 ms |

Tiếp (26/10, sáng):

| Việc | Kết quả |
| --- | --- |
| Lưu dữ kiện Python/JS xuống `~/.cache/codetrace` giữa các phiên (kiểm từng file theo mtime+size; file thêm/xoá đọc lại importer như `sync`; khoá theo hash nội dung driver, phiên bản runtime, cấu hình phân giải/project) | hermes lần đầu của phiên mới 14,5 s → 0,96 s (13 MB); oh-my-pi TS 11,2 s → 1,28 s (9 MB); sửa/thêm file lúc server tắt: kết quả giống hệt chạy không cache (Python và JS, kể cả import trước đó lạc nay phân giải được). `status` báo dung lượng, `uninstall` xoá |
| PHP | câu đầu depth 1 đã 1,7 s (warm-up 30 s chạy nền, câu hỏi chỉ đọc phần cần); chưa lưu xuống đĩa vì dữ kiện phụ thuộc Reflection của file khác |
| Generic bound trên codex (`T: SessionTask`) | đúng: `task.kind()` ở `tasks/mod.rs:313` đi qua blanket impl `T::kind` (`likely`, tầng 1), repo tí hon ra `likely` trực tiếp |
| codex `ReviewTask::kind`: 108 s, 8 GB, 56 dòng `err.kind()` của std thành `unresolved` | đồ thị crate đọc từ `Cargo.toml` loại lời gọi trong crate không phụ thuộc crate đích; ứng viên rải trên >40 file hỏi definition thay vì tô màu cả file; lỗi definition được hỏi lại: 41 s, 6,4 GB, 24 dòng `unresolved` còn lại đều là code `cfg(windows)`/macOS hoặc trong đối số macro |

Hồi quy sau các bản sửa: PHP 6/6 khớp server mới (24–61 ms); Rust tí hon và personalization-engine giữ nguyên.

`Deref` ngầm theo kiểu thật: hỏi rust-analyzer kiểu của tên cuối của toán hạng sau `*` (hover). Repo tí hon: `*b` (kiểu khai báo), `*x` (kiểu suy ra, trước bị sót), `*h.inner` (qua trường) đều `likely`; `*v * w` trên `&u32` bị loại. Hồi quy Rust giữ nguyên.

Driver JS khởi động (repo Laravel 3.000 file JS, có cache): 623 → 525 ms nhờ dùng lại kết luận "file bundle" theo (mtime, size) thay vì đọc 64 KB đầu mỗi file; phần còn lại là nạp parser TypeScript (116 ms) và giải nén cache. Cache JS vẫn khớp chạy không cache (debtnote thêm file khi server tắt).

Còn mở (Rust): auto-deref khi gọi method (`my_box.method()` của kiểu bên trong) và deref coercion ở đối số; `*x` trong file không nhắc tên kiểu đích. (`Cargo.toml` ở thư mục con đã được theo dõi sẵn: lõi quét mọi file không bị ignore, driver nhận `…/Cargo.toml` ở mọi cấp.)

Còn mở: một câu hỏi lớn chặn câu khác cùng repo (driver xử lý tuần tự); `for x in f()` ghi nhãn `reference`; `__getattr__` PEP 562; decorator chỉ bọc (`@retry`) vẫn thành entry point; câu Python đầu ở repo nhiều ngôn ngữ khởi động cả driver JS (582 MB).

## Hợp đồng driver (chung cho mọi ngôn ngữ)

- `probe`: tự kiểm tra năng lực và hành vi engine trên máy hiện tại, trả chế độ chạy.
- `inputs`: tập file/thư mục mà kết quả phụ thuộc (để lõi kiểm độ mới theo mtime + ctime + inode).
- `resolve`, `locate`, `callers`: như hiện tại.
- Tuỳ chọn `"cache": ["callers"]` trong `driver.json`: core giữ câu trả lời; kết quả `sync` của driver phải có `invalidated` (các symbol có câu trả lời đã cũ). Driver không khai báo thì luôn được hỏi lại.
- Symbol của ngôn ngữ khác (`file#tên`) được driver PHP/Laravel trả rỗng ngay, không quét.

## Thiết kế

- `scripts/codetrace/` — lõi Node/TS (chạy thẳng bằng Node 22, không dependency).
- Lõi trung tính: chỉ biết symbol, edge, confidence. Không biết PHP hay Laravel.
- Mỗi driver là một thư mục `drivers/<name>/` có `driver.json` (điều kiện nhận diện + lệnh chạy) và một tiến trình nói JSON lines qua stdin/stdout. Thêm driver = thêm thư mục, lõi không đổi.
- Driver dùng công cụ chính chủ của ngôn ngữ/framework:
  - `php`: `PhpToken` tìm chỗ gọi, Reflection biết kiểu, `rg` lọc file ứng viên.
  - `laravel`: boot app, đọc router, event dispatcher (gồm observer), job, command.
- Mỗi kết quả có `confidence`: `exact`, `likely`, `unresolved`. Không bỏ ứng viên trong im lặng.
- Mỗi dòng kết quả kèm `file:line#hash` (hash nội dung dòng) để neo vị trí.

## Giai đoạn

1. Lõi + driver PHP + driver Laravel, nghiệm thu trên `repos/easy-ai-platform-backend`.
2. Driver TS/Vue (TypeScript compiler API), thử trên backend và widget.
3. Driver Rust (cần `rustup component add rust-analyzer`, hỏi trước), thử trên personalization-engine.

## Nghiên cứu cách runtime tự nạp (2026-10-25)

Đo trên `repos/easy-ai-platform-backend`, PHP 8.4.27 (có `pcntl`, `Zend OPcache`).

- Laravel boot đọc đúng 1.607 file PHP (338 `app/`, 63 `config/`, 12 `routes/`, còn lại vendor) và quét 13 thư mục (`app/Listeners`, `app/Console/Commands/*`, `config`, `database/migrations`). Ghi bằng `strace` qua REA, evidence `ev_d12638f7a8680262aed4ee406cc00df7095b7831f9b1f2f589ff34c77727c511`. Đây chính là tập đầu vào quyết định wiring: chỉ cần boot lại khi một trong số đó đổi.
- Composer: classmap 50.888 class (2.791 `App\`), thiếu thì rơi về PSR-4 (`App\` → `app/`, `ClassLoader::findFile`). Lõi Rust tự tra class → file được, không cần PHP.
- Engine PHP: class đã nạp không nạp lại được trong cùng tiến trình (Reflection vẫn thấy bản cũ, include lại thì fatal "Cannot redeclare class").
- Cách chính chủ để vẫn dùng Reflection mà luôn mới: tiến trình cha chỉ nạp vendor, mỗi lần tra `pcntl_fork` một con. Với `opcache.enable_cli=1`, `revalidate_freq=0`, bộ nhớ opcache chia sẻ giữa các con và kiểm mtime từng file; file sửa trong 2 giây gần nhất không bị cache (`file_update_protection`).
  - Boot Laravel trong con: 62–68 ms (lạnh: 210–415 ms).
  - Reflection 300 class `App\Services`: 21 ms (lần đầu 183 ms).
  - Fork rỗng: 19–28 ms.
  - Thí nghiệm sửa file giữa các lần fork: con thấy ngay bản mới (`a` → `a,b` → `c`).
- TypeScript làm cùng mẫu: mỗi request `synchronizeHostData`, file nào `getScriptVersion` không đổi thì dùng lại `SourceFile` cũ (`typescript.js:152762`, `:152911`); tìm references = `indexOf` tên rồi `checker.getSymbolAtLocation` (`:173538`, `:173635`).

### Độ mới khi code thay đổi

- Mã nguồn `php-8.4.27` (`ext/opcache/ZendAccelerator.c`): opcache chỉ so `st_mtime` 1 giây (`:1090`, `:1147`); `revalidate_freq=0` thì kiểm mỗi lần nạp (`:1174`); file có mtime mới hơn `request_time - file_update_protection` không được cache (`:1794`), và `request_time` đặt một lần mỗi request (`:2655`), nên con fork kế thừa thời điểm của cha.
- Thí nghiệm fork + opcache với cha sống lâu: thêm method, đổi kiểu trả về, hai lần ghi trong cùng giây, thêm file, xoá file đều cho kết quả mới. Ghi giữ nguyên mtime cũ (`touch -d @old`) trả bản cũ; kiểm thêm `ctime` + inode rồi `opcache_invalidate` thì đúng.
- Đối chiếu binary `~/.local/php/8.4.27/bin/php` (SHA-256 `5001bf47…`, ELF tĩnh, bị strip) với mã nguồn: Ghidra 12.1.4 qua REA tìm chuỗi `opcache_invalidate` tại `0x7fcb2f` (evidence `ev_62a238044e261bce73086379c225f85447ec44d96c1a19d83d90ba7a3f435d07`, phân tích mất 44 phút, ~1,9 GB RAM). Project Ghidra bị xoá khi lệnh kết thúc, nên phần hàm đọc bằng relocation + `objdump`:
  - `zif_opcache_invalidate` (`0x3da4450`, tìm qua relocation của bảng hàm): `zend_parse_parameters` với `"S|b"`, kiểm `restrict_api`, gọi `zend_accel_invalidate`, trả `IS_TRUE`/`IS_FALSE` — khớp `zend_accelerator_module.c:917`.
  - `zend_accel_invalidate` (`0x3db69a0`): `force` → `validate_timestamps` → `do_validate_timestamps` → gỡ script — khớp `ZendAccelerator.c`.
  - `do_validate_timestamps` (`0x3db5ee0`): so tên file, `timestamp == 0` thì thất bại, so `zend_get_file_handle_timestamp(..., NULL)` với `persistent_script->timestamp` — khớp `:1113`.
  - `zend_get_file_handle_timestamp` (`0x3db5b10`): trả `st_mtime` (offset `0x58` của `struct stat`), kích thước từ `st_size` (`0x30`) — khớp `:1021`.
- Laravel boot còn đọc `.env`, `composer.json` và 5 file trong `storage/framework/cache/data/`: tập đầu vào của wiring phải là mọi file boot mở, không chỉ file PHP.
- Vendor nạp sẵn ở cha: đổi `composer.lock`/`vendor/composer/installed.json` thì phải khởi động lại cha.

## Bản thử daemon (2026-10-25)

Lõi Rust: daemon theo repo, quét dấu vân tay (mtime/ctime/inode/size) mỗi lần tra (10.495 file: 7–8 ms một luồng), báo file đổi cho driver qua `sync`, tự khởi động lại driver, báo CPU/RAM mỗi lần tra, không giới hạn tầng.
Driver PHP: cha chỉ nạp autoload + opcache, giữ dữ kiện file/class + sổ phụ thuộc; mọi Reflection chạy trong con fork. Driver Laravel: wiring boot trong con, giữ tới khi file boot đã đọc đổi.

Lần tra mẫu trên backend: một method nằm trong file service lớn (~1 MB), cây gọi 414 dòng, 193 symbol, sau khi thêm chỉ mục theo tên/class, cache kết quả và dừng sớm theo câu hỏi:

| Tình huống | Wall | CPU | RAM (server + 2 driver) |
|---|---|---|---|
| Lạnh, lần đầu | 16–17 s | 44–145 s (dao động, chưa rõ vì sao) | ~540 MB |
| Ấm, không đổi gì | 25–30 ms | 90–120 ms | ~545 MB |

Sửa nội dung thật trên bản sao backend; mỗi kết quả ấm so với daemon khởi động mới trên cùng trạng thái code:

| Sửa | Wall (ấm) | CPU | Kết quả so với daemon mới |
|---|---|---|---|
| S1 thêm comment cuối file service lớn | 827 ms | 1,05 s | giống hệt |
| S2 chèn một dòng phía trên khai báo class | 841 ms | 1,07 s | giống hệt |
| S3 thêm câu lệnh trong một action của controller | 480 ms | 0,53 s | giống hệt |
| S4 thêm route vào file route | 460 ms | 0,53 s | giống hệt (thêm điểm vào mới) |
| S5 một setter trong chuỗi gọi đổi từ `return $this` sang `return null` | 1.165 ms | 1,36 s | giống hệt (cây rút còn 154 dòng) |
| S6 thêm file mới có lời gọi tới method đó | 808 ms | 1,78 s | giống hệt (caller mới xuất hiện) |

Vòng tăng tốc sau khi sửa (đo từng pha bằng `CODETRACE_PROFILE`):
- Dừng sớm theo đơn vị code: mỗi câu hỏi gửi engine ghi các đơn vị nó dựa vào (`shape` của class = mọi thứ trừ thân method; `body` của method). File đổi được đọc lại trước; chỉ câu hỏi dựa vào đơn vị đổi mới được hỏi lại.
- Làm mới theo lời gọi: mỗi lời gọi ghi token và câu hỏi nó dùng; câu trả lời đổi thì chỉ tính lại các lời gọi đó.
- Cache kết quả xoá theo file/tên/class đã dùng. Lõi hỏi driver song song; Laravel boot lại wiring ngay trong `sync`.
- `opcache.file_update_protection=0`: lõi tự invalidate mọi file đổi; bảo vệ 2 giây làm file vừa sửa bị biên dịch lại ở mọi tiến trình con.
- Rút trước toàn repo ở nền khi daemon khởi động: 3 tiến trình con `nice 19`, lô 10 file; lô chết được tách từng file, file làm PHP dừng được báo thành cảnh báo.

Rút trước trên backend: ~42–46 s, 5.158 file, 105–122 s CPU; RAM thật (PSS) đỉnh ~1,1 GB, sau đó server + driver ~670 MB. 1 file test làm PHP dừng (một file test), được báo ra.

Vòng tiếp (đo từng pha cả lõi lẫn driver):
- Fork một tiến trình cha ~600–800 MB tốn 15–20 ms, chờ thu dọn thêm 15–20 ms, trong khi việc thật < 1 ms → nhóm tiến trình con fork sẵn (dùng một lần, fork bù khi rảnh 5 ms), thu dọn trễ; con tự thoát khi cha chết.
- Đọc lại file: tái dùng lời gọi của method có body + shape + head không đổi và câu hỏi không dựa unit đổi; tái dùng cả câu trả lời kế thừa.
- Quét: chỉ stat lại file/thư mục đã biết (4 luồng), thư mục đổi thì liệt kê lại, .gitignore đổi thì duyệt lại hết.
- Opcache 256 MB bị đầy sau rút trước → tắt opcache trong tiến trình nền; trần 512 MB + 64 MB interned string (dùng thật ~200–217 MB).
- Rút trước dữ kiện class ở nền (~5 s, ~4.700 class). Dữ kiện class chỉ giữ method tự khai báo; method kế thừa tra theo chuỗi cha → RAM driver 1.162 → 674 MB.
- Laravel: lần boot nền gắn hook `opentelemetry` vào mọi hàm của file boot đã nạp để ghi hàm nào chạy lúc boot (437 file, 272 hàm chạy). Chỉ boot lại khi: đổi ngoài thân hàm/`use`, đổi thân hàm đã chạy lúc boot (method trait luôn tính là đã chạy), file mới/bị xoá trong thư mục boot đọc, `.env` đổi, hoặc chưa có bản ghi. Trait `Dispatchable` của class dự án tính sẵn ở nền theo lô 100.

Sau khi rút trước xong (6 lần sửa thật; mỗi kết quả so với daemon mới — đều giống hệt; kết quả lạnh giống hệt mốc của bản code trước):

| Sửa | Lần 1 | Giờ |
|---|---|---|
| Không đổi gì | — | 27 ms |
| S1 thêm comment cuối file service lớn | 827 ms | 49 ms |
| S2 chèn một dòng phía trên khai báo class | 841 ms | 79 ms |
| S3 thêm câu lệnh trong một action của controller | 480 ms | 59 ms |
| S4 thêm route (bắt buộc boot lại Laravel) | 460 ms | 197 ms |
| S5 một setter trong chuỗi gọi đổi từ `return $this` sang `return null` (213 lời gọi ở 18 file đổi kiểu thật) | 1.165 ms | 177 ms |
| S6 thêm file mới có lời gọi tới method đó | 808 ms | 43 ms |

Tài nguyên: rút trước file ~42 s (~124 s CPU, nice 19) + class ~5 s; PSS đỉnh ~1,5 GB lúc rút trước, sau đó ~505 MB (server + driver). Mỗi lần boot lại Laravel kéo theo một lần boot có hook ở nền (~0,7 s CPU).

Còn lại trên 100 ms: S4 (boot lại Laravel ~72 ms + tính lại), S5 (làm mới 213 lời gọi).

## Nghiệm thu giai đoạn 1

10 method thật, chọn có chủ đích: gọi qua interface, Eloquent static, job dispatch từ controller, listener chỉ tới được qua event, observer, tên phổ biến (`handle`/`create`), gọi qua `$this->prop`, gọi qua biến cục bộ.
Mỗi method ghi: số file `rg` khớp, số kết quả theo confidence, số caller thật đối chiếu tay, chỗ sót và chỗ báo sai.
