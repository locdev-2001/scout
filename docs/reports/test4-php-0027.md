# Test codetrace vòng 4, đợt A: PHP/Laravel so với `rg` (26/10/2026 00:27)

Chỉ đọc 4 repo (không sửa, không commit, không chạy test). Lệnh chuẩn: `codetrace callers '<đích>' --depth 1 --compact`, đối chiếu `rg -n` rồi đọc tay từng dòng. Thời gian ghi dạng "Cost server / thời gian thực của lệnh". Kích thước output đo **sau khi bỏ** dòng `Cost:` và cảnh báo `vite.config.ts` (vẫn tính dòng `Total`, `… stopped`).

## Thực tế 4 repo khác với đề bài

| Repo | File PHP | `vendor/` | Kết quả |
|---|---|---|---|
| mamiviet | 248 | có | Phân tích PHP/Laravel đầy đủ. Toàn bộ so sánh có ý nghĩa nằm ở repo này |
| vitalnoni-v2 | 993 | **không** | Driver PHP và Laravel chết ngay khi khởi động. Mọi truy vấn PHP ra `Direct callers (0)`, rc=0 |
| dash_yona | 8.119 (6.464 nằm trong `application/vendor`) | `application/vendor` | Perfex/CodeIgniter, không có `composer.json` ở gốc, nên chạy bằng driver chữ (generic). Mọi dòng đều là `[text call, unresolved]` |
| hoi.cf | **668** (không phải ~41.000: con số đó tính cả `vendor/`, mà repo chưa có `vendor/`) | **không** | Giống vitalnoni. **Bài chịu tải không thực hiện được**, vì thiếu vendor thì driver PHP không chạy |

RAM lúc bắt đầu: 6 GB trống, 12 GB available, đủ. Sau mỗi repo tôi đều `codetrace stop`.

## Tóm tắt

- **mamiviet (có vendor), 17 đích (16 tính tỉ lệ, bỏ m16):** codetrace **hơn rg 6**, **ngang 8**, **kém 2**. Ở đây chưa thấy `exact` nào sai. Phần nối theo quy ước Laravel đã chạy thật: middleware nối ra 16 route, observer nối ra `event eloquent.saved`, `Artisan::call('x')` và `schedule->command('x')` nối về `handle` của lệnh, scope nối về `->published()`. Đó là những thứ rg không làm được. Truy vấn nhanh: ấm 1–15 ms server / 20–36 ms CLI. Lần đầu mất 725 ms. Không giới hạn depth trên `Setting::get` mất 54 ms. Khi ấm, codetrace mất 20–36 ms CLI so với 10–12 ms của rg: chậm hơn 2–3 lần nhưng cùng bậc.
- **Repo thiếu `vendor/` (vitalnoni-v2, hoi.cf): codetrace kém rg hoàn toàn.** Nó trả `Direct callers (0)`, rc=0, kèm cảnh báo PHP fatal trên stderr. Trong khi đó rg thấy ngay chỗ gọi (`WalletController.php:96`, `routes/api.php:111,150`). `locate` thì ra `Not found` và **từ lần hỏi thứ hai mất luôn cảnh báo**. Không có cơ chế lùi về tìm theo tên, dù repo không có `composer.json` (easy-ai-dev, dash_yona) thì lại có.
- **dash_yona (generic): kém rg.** Với `get_option` nó ra đúng 1.293 dòng như rg, nhưng output 297 KB so với 197 KB của rg (mỗi dòng in hai lần), và không có tên hàm chứa. `Clients_model::get` ra 2.331 dòng (497 KB) vì bỏ qua phần tên class. Thật ra chỉ 22 dòng `->clients_model->get(` mới có khả năng đúng. `locate` thì ra `Not found` với mọi dòng PHP.
- **Tên không tồn tại vẫn ra `Direct callers (0)`, rc=0**, trái với README ("ra `Not found`").
- **MCP của phiên này hỏng hẳn:** cả 3 lệnh gọi (`callers`, `locate`, với repo bất kỳ) đều trả `cannot start server: No such file or directory (os error 2)`.
- **Hook tìm kiếm gần như vô dụng với lệnh Bash:** nó luôn hỏi repo theo `cwd` của phiên, bỏ qua `cd X &&` và đường dẫn truyền cho `rg`. Nó còn bắt cả từ khoá `function` làm tên hàm. Hook sau khi sửa (Edit) thì có ích thật.

Tính trên mọi đích đã chạy (16 mamiviet + 3 vitalnoni + 2 hoi.cf + 3 dash_yona = 24): **hơn 6, ngang 8, kém 10**.

## Bảng đích

### mamiviet (có vendor, 248 file)

| # | Đích | rg (đúng / sai) | codetrace (đúng / sót / thừa) | Nhãn | ms server / CLI | Cỡ ct vs rg | Kết luận |
|---|---|---|---|---|---|---|---|
| m1 | `PostContentNormalizer::looksLikeTiptapDoc` (static, tên hiếm) | 2 / 0 (+1 dòng khai báo) | 2 / 0 / 0 | exact | **725 / 750 (lạnh)**; 2 / 20 (ấm) | 0,6 KB / 0,3 KB | ngang |
| m2 | `Post::scopePublished` (scope) | 5 / 0, nhưng rg **không phân biệt** với `Page::scopePublished` (`Page.php:25`) | 5 / 0 / 0; `Page::scopePublished` = 0 (đúng) | **unresolved** cả 5, dù đều là `Post::query()->published()` | 15 / 32 | 1,0 / 0,5 KB | hơn (tách được Post/Page), nhãn yếu |
| m3 | `setting` (helper toàn cục, `app/Helpers/Helper.php:6`) | 0 lời gọi thật (9 dòng trong comment `config/mail.php:49–126`) | hỏi theo tên: **ra class `App\Models\Setting`**, 0 caller; hỏi `Helper.php:8` thì ra `setting`, 0 caller (đúng) | — | 2 / 22 | 0,15 / 0,9 KB | **kém**: tên trùng class (khác hoa thường) bị chọn nhầm, không báo mơ hồ |
| m4 | `Setting::get` (static, tên phổ biến) | 22 / 0 | 22 / 0 / **4** (`->get()` của Eloquent builder ở `BlogFeedController.php:35`, `MenuController.php:29,75`, `PostController.php:87`) | 10 hàm exact, 4 unresolved | 2 / 21 | 2,8 / 2,6 KB | ngang (có tên hàm chứa nhưng thêm 4 dòng nhiễu, mỗi dòng in 2 lần) |
| m5 | `HandlesTranslatableFormData::applyTranslations` (trait) | 0 (trait không ai `use`) | 0 | — | 2 / 20 | — | ngang (code chết, cả hai đều ra 0) |
| m6 | `PostObserver::saved` (đăng ký `Post::observe` ở `AppServiceProvider.php:21`) | 0 lời gọi (chỉ thấy dòng `observe`) | `event eloquent.saved: App\Models\Post` | exact | 2 / 22 | 0,3 KB | **hơn** |
| m7 | `SetLocale::handle` (middleware alias `setlocale`, `Kernel.php:67`) | 2 dòng `Route::middleware('setlocale')` | `middleware on 16 routes` (2 api + 14 web, đếm tay khớp) | exact | 2 / 21 | 0,4 KB | **hơn** |
| m8 | `InstagramPostService::list` (tên phổ biến, gọi qua `$this->service`) | 1 / 0 | 1 / 0 / 0 | **unresolved**, dù property được gán từ tham số constructor có kiểu (`InstagramPostController.php:14–17`) | 2 / 22 | 0,55 / 0,1 KB | ngang, nhãn yếu |
| m9 | `InstagramPostController::index` (action route) | 2 route ở `routes/api.php:7` | route `GET /api/user/instagram-posts` + dòng `[callable]` có caller tên `routes/api.php:6` (closure group) | exact | 4 / 21 | 0,6 KB | hơn |
| m10 | `RegenerateSitemap::__construct` (job, không khai báo constructor) | 2 dòng `::dispatch()` (`PostObserver.php:47`, `NormalizePostContent.php:67`) | đủ, nhưng ở depth 1 chỉ hiện `RegenerateSitemap::dispatch [dispatch]` không có file:dòng; dòng gọi thật nằm dưới "Further up" và trỏ về **dòng khai báo hàm** (`:17`, `:42`) chứ không phải dòng gọi | exact | 5 / 23 | 0,54 / 1,2 KB | **kém nhẹ** (phải hỏi thêm mới thấy dòng gọi) |
| m11 | `InstagramPostController::__construct` (container dựng khi chạy route) | 0 | 0 | — | 2 / 20 | — | ngang (lẽ ra nên ra 2 route) |
| m12 | `Setting::set` | 4 / 0 | 4 / 0 / 0 (gom thành 2 hàm) | exact | **371 / 410 (lạnh lần 2)** | 0,55 / 0,4 KB | ngang |
| m13 | `PostResource::form` (Filament gọi) | 0 | 0 | — | 15 / 36 | — | ngang (Filament chưa được coi là lối vào) |
| m14 | `PostApiResource::collectionForLocale` | 3 / 0 | 3 / 0 / 0 | exact | 3 / 24 | 0,8 / 0,5 KB | ngang |
| m15 | `GenerateSitemapCommand::handle` (lệnh artisan) | phải tìm chuỗi `sitemap:generate`: 4 dòng app + 4 dòng test | 4 / 0 / 0 (`schedule`, 2 `Artisan::call`, `artisan sitemap:generate`), 4 dòng test chỉ đếm | exact | 12 / 30 | 0,8 / 0,7 KB | **hơn**, nhưng thiếu lối vào (lỗi T4) |
| m16 | `handle` (tên phổ biến) | — | báo "matches several symbols" kèm 7 ứng viên | — | — | — | hơn (rg ra mọi `handle` của vendor) |
| m17 | `MenuItem::scopeAvailable` (scope, có 1 chỗ gọi trong closure `with([... fn ($q) => $q->available()])`) | 4 / 0 | 4 / 0 / 0 | unresolved cả 4 | 14 / 32 | 0,8 / 0,3 KB | **hơn** (rg cũng ra 4 nhưng không biết đó là scope nào) |

Ghi chú: mamiviet có hơn 6 (m2, m6, m7, m9, m15, m17), ngang 8 (m1, m4, m5, m8, m11, m12, m13, m14), kém 2 (m3, m10). m16 không tính vào tỉ lệ. Ngoài bảng còn hai chỗ kém nằm ở phần `locate` (closure lồng, T3; closure sự kiện model, T5).

### vitalnoni-v2 và hoi.cf (thiếu `vendor/`)

| # | Đích | rg | codetrace | ms | Kết luận |
|---|---|---|---|---|---|
| v1 | `WalletService::transfer` | 1 (`app/Http/Controllers/Api/Store/WalletController.php:96`) | `Direct callers (0)`, rc=0; stderr: `Warning: laravel failed … missing …/vendor/autoload.php: run composer install` + `php failed …` kèm nguyên stack trace PHP | 140 / 158 (mỗi lần đều `a driver restarted`) | **kém** |
| v2 | `save` (tên phổ biến) | hàng trăm dòng | chỉ liệt kê 8 hàm `save` trong `.vue`, **không có method PHP nào**; rc=1 | 2.032 / 2.068 (lạnh) | **kém** |
| v3 | `locate app/Services/WalletService.php:40` | — | lần 1 `Not found` + cảnh báo; **lần 2 chỉ còn `Not found` và `a driver restarted`, không có cảnh báo** | 47 | **kém** (im lặng) |
| h1 | `ForgotPasswordController::forgotPassword` | 2 route (`routes/api.php:111,150`) | `Direct callers (0)`, rc=0, cùng cảnh báo | 480 / 514 (lạnh) | **kém** |
| h2 | `locate …/ForgotPasswordController.php:25` | — | `Not found`, không có cảnh báo | 56 | **kém** |

### dash_yona (CodeIgniter, chạy bằng driver chữ)

| # | Đích | rg | codetrace | ms | Cỡ | Kết luận |
|---|---|---|---|---|---|---|
| d1 | `get_option` (helper toàn cục, `application/helpers/settings_helper.php:45`) | 1.296 dòng = 1.293 lời gọi + 3 dòng khai báo (`settings_helper.php:45`, `App.php:289`, `App_sms.php:67`), 13 ms | 1.293 dòng `[text call, unresolved]`, **trùng khớp từng dòng** với tập lời gọi của rg (đã so bằng `comm`). Trộn thêm 14 dòng gọi **method khác** cùng tên: `$this->get_option` ở `Sms_msg91.php:17–19`, `$this->app_sms->get_option` ở `views/admin/settings/includes/sms.php:26,47,64`, `$CI->app->get_option` ở `settings_helper.php:53` | **2.584 / 2.656 lạnh** (đọc 8.119 file); 21 / 41 ấm | **297 KB** so với 197 KB | **kém** (to hơn rg 1,5 lần, không có tên hàm chứa) |
| d2 | `Clients_model::get` | 22 dòng `->clients_model->get(` | **2.331 dòng** mọi `get(` trong repo (bỏ qua phần class). `App::get_option` cũng ra đúng 1.293 dòng như `get_option` | 37 | **497 KB** | **kém** nặng |
| d3 | `locate` ở `Clients_model.php:50`, `client_js.php:117`, `settings_helper.php:50` | — | cả 3 đều `Not found` | — | — | **kém** |
| d4 | Không giới hạn depth trên `get_option` | — | 2.289 ms (lạnh), 1.297 dòng, cây phẳng (không có tầng nào phía trên) | — | 284 KB | — |

## `locate` (mamiviet)

| Dòng | Kết quả | Đánh giá |
|---|---|---|
| `routes/web.php:21` (`Artisan::call` trong closure của `Cache::lock()->block(10, function…)`, closure này nằm trong closure route `/sitemap.xml`) | `routes/web.php:19` | đúng closure trong cùng, nhưng tên không cho biết nó thuộc route nào. **`callers` ra 0** (lỗi T3) |
| `app/Models/Post.php:78` (trong `static::saving(function …)` ở `:75`) | `App\Models\Post::booted` | gán cho `booted`, không phải closure sự kiện. `callers` ra 0 (lỗi T5) |
| `app/Models/Post.php:30` (thuộc tính) | `App\Models\Post` | hợp lý |
| `app/Support/SeoBuilder.php:156` (trong closure `$tr` ở `:154`) | `SeoBuilder::forMenuItem` | hợp lý |
| `resources/views/app.blade.php:5` | `resources/views/app.blade.php:1` (cả file) | chấp nhận được |
| `routes/web.php:17`, `:11` | `routes/web.php:16`, `:10`; callers = `route GET /sitemap.xml`, `route GET /robots.txt` | **đúng**: closure route nay đã thành lối vào |
| `routes/web.php:30` (closure `->group()`) | `routes/web.php:28`, callers 0 | không còn lối vào giả như vòng 3 |

Mọi lần `locate` ấm đều 10 ms.

## Không giới hạn depth

- mamiviet `Setting::get`: 54 ms, 118 dòng, 14,6 KB, 18 lối vào. Lối vào giả: `routes/web.php (top-level code: runs when the file is loaded)`, có từ dòng callable `Route::get('/blog/feed.xml', [BlogFeedController::class,'de'])` ở `routes/web.php:62–63`. Dòng đăng ký route bị tính như code chạy lúc nạp file. Phía admin Filament (`JsonLdSettings::mount/save`, `InstagramPostResource::table`) dừng mà không có lối vào nào.
- mamiviet `GenerateSitemapCommand::handle`: 21 ms, 6 lối vào. **Thiếu `route GET /sitemap.xml`** (`routes/web.php:21`) và **thiếu lịch chạy** (`Kernel.php:16` `->daily()` hiện là caller nhưng không vào "Affected entry points").
- dash_yona `get_option`: xem d4.
- Không truy vấn nào treo.

## Hai hook

**Hook trước khi tìm (before-search):**
- **Sai repo.** Với lệnh Bash, repo luôn lấy theo `cwd` của phiên (`src/hook.rs:75`). Nó bỏ qua `cd /repo && rg …` và cả `rg pat /repo/path`. Trong cả bài test, phiên ở `easy-ai-dev`, còn mọi `rg` của tôi chạy trong 4 repo kia, nên hook **chưa lần nào** trả lời đúng repo đang tìm. Tác dụng phụ: nó tự khởi động daemon cho `easy-ai-dev` (RAM), một repo tôi không hề hỏi tới.
- **Bắt nhầm từ khoá.** `rg -n 'function ' …` sinh ra "who calls `function`" kèm 2 dòng `array_map(function …` trong `plans/…/import-ngoc-an-case.php`. Việc này lặp lại 4 lần trong phiên. Nguyên nhân: `searched_name` gọi `trim()` trước, nên tiền tố `"function "` (có dấu cách) không còn khớp, và chữ `function` lọt qua bộ lọc định danh.
- Mẫu `Setting::get\(` hay `Class::method` không kích hoạt hook (có `::` ở giữa), trong khi đây chính là lúc thông tin callers có ích nhất.
- Thời gian (gọi tay `codetrace hook before-search`): ấm 16–23 ms; daemon lạnh 840 ms (mamiviet) và 2.360 ms (dash_yona). Ngân sách 3 s nên không treo, nhưng trên repo lạnh mỗi lần grep chậm thêm tới ~2,4 s.
- Trên repo generic (dash_yona), hook chèn 40 dòng `[text call, unresolved]` (7 KB), y hệt thứ `rg` sắp in ra. Đây là nhiễu thuần.

**Hook sau khi sửa (after-edit), thử trên bản sao mamiviet trong scratchpad (copy cả `.git` và `vendor`; đã kiểm `ReflectionClass` trỏ đúng file trong bản sao):**
- Sửa thân `Setting::get` (`$value = self::raw(trim($key))`) thì hook trả đủ 14 caller trực tiếp. **Có ích**, nhưng mang theo cả 4 dòng nhiễu `->get()`, và mỗi dòng nhiễu xuất hiện 2 lần (danh sách caller và mục "Unresolved, read by hand").
- Đổi chữ ký `HtmlSanitizer::clean(?string $html, string $profile = 'blog')` thì hook ra đúng 2 caller (`MenuItem.php:97`, `Post.php:85`, khớp rg) và báo "13 callers inside tests". **Rất có ích** đúng lúc đổi chữ ký.
- Không thấy chậm khi dùng: daemon đã ấm.
- Giới hạn có sẵn trong code: hook không chạy với file nằm ngoài repo git (`repo_root` cần `.git`). Nó cũng không chạy khi đoạn mới xuất hiện hơn một lần trong file.

## Lỗi theo mức nặng

### Nặng

1. **T1: Repo có `composer.json` mà thiếu `vendor/`: mọi câu PHP ra 0 caller, rc=0.** Bằng chứng: `vitalnoni-v2` `WalletService::transfer` (rg: `app/Http/Controllers/Api/Store/WalletController.php:96`), `hoi.cf` `ForgotPasswordController::forgotPassword` (rg: `routes/api.php:111,150`). Cảnh báo chỉ nằm trên stderr, dạng stack trace PHP thô. Ai (hoặc agent nào) chỉ đọc stdout sẽ thấy `Direct callers (0)`. Không có cơ chế lùi về tìm theo tên, trong khi repo không có `composer.json` thì có (easy-ai-dev, dash_yona). Mỗi câu hỏi còn khởi động lại driver (`a driver restarted`, ~140 ms).
2. **T2: `locate` trên repo hỏng driver ra `Not found`, từ lần hai thì im lặng** (`vitalnoni-v2 app/Services/WalletService.php:40`, `hoi.cf …/ForgotPasswordController.php:25`). Trên repo generic, `locate` ra `Not found` với mọi dòng PHP (dash_yona d3).
3. **T6: MCP hỏng mà không có cách hiểu:** cả 3 lời gọi đều `cannot start server: No such file or directory (os error 2)`. Nguyên nhân thấy được: tiến trình `codetrace mcp` của phiên (pid 1924826 và 1925179, chạy từ 23:05) dùng binary cũ `easy-ai-dev/.cache/codetrace-target/release/codetrace`, nay đã bị xoá (`/proc/<pid>/exe … (deleted)`). Khi spawn daemon nó dùng chính exe đó. Sau khi `codetrace update` hoặc chuyển chỗ cài, mọi phiên đang mở mất công cụ, chỉ còn lại một thông báo khó hiểu. Agent dùng MCP (cách được khuyên dùng) không nhận được gì.
4. **T7: Tên không tồn tại ra `Direct callers (0)`, rc=0** (`PostContentNormalizer::nope`, `nopeFunctionXyz` trên mamiviet), trái README. Agent sẽ hiểu là "không ai gọi, sửa thoải mái".

### Trung bình

5. **T3: Closure truyền làm đối số bên trong closure route mất đường lên route.** `routes/web.php:21` (`Cache::lock(...)->block(10, function () use ($path) { Artisan::call(...) })` trong route `/sitemap.xml`) cho `callers` = 0. Hệ quả: `GenerateSitemapCommand::handle` thiếu lối vào `route GET /sitemap.xml`.
6. **T4: Lịch chạy không thành lối vào.** `app/Console/Kernel.php:16` `$schedule->command('sitemap:generate')->daily()` hiện ra như caller `[schedule]`, nhưng "Affected entry points" không có dòng lịch, và `Kernel::schedule` cũng không có caller.
7. **T5: Closure sự kiện model trong `booted()` ra 0.** `Post.php:75` `static::saving(function …)`: `locate :78` gán cho `Post::booted`, còn `callers` = 0. Observer thì nối được tới `event eloquent.saved`, nhưng cách viết `static::saving(fn)` lại không.
8. **T8: Tên hàm toàn cục trùng tên class bị hiểu thành class, không báo mơ hồ.** `codetrace callers setting` và `\setting` ra `App\Models\Setting` (class) với 0 caller. Hàm `setting()` ở `app/Helpers/Helper.php:6` chỉ hỏi được theo `path:line`. Hỏi một class (`Setting`) cũng ra "0 caller" dù `Setting::get` được gọi 22 chỗ: không rõ "caller của class" nghĩa là gì.
9. **T9: Driver chữ bỏ qua phần class và bỏ qua `->`.** `Clients_model::get` ra 2.331 dòng (497 KB), so với 22 dòng có khả năng đúng. `App::get_option` ra cùng 1.293 dòng với hàm toàn cục `get_option`. `get_option` toàn cục kéo cả `$this->get_option` (`application/libraries/sms/Sms_msg91.php:17`). Với rg, chỉ cần `->clients_model->get\(` là gọn hơn hẳn.
10. **T10: Repo con có composer (`application/composer.json` + `application/vendor/autoload.php` của Perfex) không được nhận ra**, nên dash_yona rơi về driver chữ.
11. **T11: Chuỗi Eloquent mất kiểu sau scope.** `Post::query()->published()` bị gắn unresolved (`PostController.php:24,66,81`, `GenerateSitemapCommand.php:50`, `BlogFeedController.php:30`). Đuôi `->get()` của chuỗi `Post::query()…` và `MenuCategory::query()…` bị kéo vào thành unresolved của `Setting::get` (4 dòng thừa ở m4), chỉ vì file đó có nhắc chữ `Setting`.
12. **T12: Property không kiểu, được gán từ tham số constructor có kiểu, ra unresolved.** `InstagramPostController.php:12–17,26` (`protected $service; … $this->service = $service; … $this->service->list(...)`).

### Nhẹ

13. Ở depth 1, job dispatch chỉ hiện `X::dispatch [dispatch]` không có file:dòng. Phần "Further up" trỏ về dòng khai báo hàm (`NormalizePostContent::handle:17`) thay vì dòng gọi (`:67`).
14. Dòng route `[callable]` lấy caller là `routes/api.php:6` hoặc `routes/web.php:28` (closure group), và được đếm thêm 1 exact ngoài chính route. Kết quả: 2 exact cho một route.
15. Mỗi dòng unresolved in 2 lần (trong "Direct callers" và "Unresolved, read by hand"). Ở dash_yona, việc này đẩy output lên 297 KB.
16. Cảnh báo `vite.config.ts could not be loaded` hiện ở **mọi** câu hỏi PHP trên mamiviet, kể cả `locate`.
17. "Some branches were cut: raise --depth" hiện ngay cả khi người dùng tự đặt `--depth 1`.
18. Filament (`PostResource::form`, `JsonLdSettings::mount/save`, `InstagramPostResource::table`) không được coi là lối vào, nên các cây dừng không có điểm kết thúc. Controller `__construct` (container dựng) ra 0.

## Chạy đúng (đáng ghi nhận)

- Middleware alias nối ra đủ 16 route, gom thành 1 dòng. Observer nối ra `event eloquent.*`. `Artisan::call` và `schedule->command` nối về `handle`. Closure route trực tiếp nối ra `route GET /x`. `->group(function)` không còn thành lối vào giả.
- Hỏi `handle` (tên mơ hồ) thì báo 7 ứng viên thay vì đoán.
- Tốc độ PHP thật rất tốt: lần đầu 725 ms cho 248 file. Ấm 1–15 ms server. Truy vấn sâu 21–54 ms.

## Đề xuất cải tiến

1. **Thiếu vendor thì nói thẳng và lùi về tìm theo tên.** Driver PHP chết lúc khởi động thì: (a) in một dòng lỗi gọn trên **stdout** (`PHP: chưa phân tích được: thiếu vendor/autoload.php, chạy composer install`), không in stack trace; (b) rc khác 0 hoặc có cờ "partial"; (c) liệt kê các dòng khớp tên dưới dạng `[text call, unresolved]` như driver generic, chứ không trả 0; (d) không khởi động lại driver ở mọi câu hỏi (ghi nhớ lỗi tới khi `vendor/autoload.php` xuất hiện). Áp dụng cả cho `locate`: với PHP, chỉ cần tokenizer, không cần autoload.
2. **Tên không tồn tại phải ra `Not found` với rc=1**, kể cả khi truy vấn có dạng `Class::method`.
3. **MCP:** khi spawn daemon, đừng dùng `current_exe()` nếu nó đã bị xoá. Hãy tìm lại `codetrace` trên PATH hoặc wrapper đã cài, hoặc báo rõ "binary đã được cập nhật, khởi động lại phiên".
4. **Hook tìm kiếm:** lấy repo từ `cd X &&` đầu lệnh, và từ đối số đường dẫn cuối của `rg`/`grep` khi có. Bỏ qua từ khoá ngôn ngữ (`function`, `class`, `return`, `public`, …), xử lý `trim` sau khi bỏ tiền tố. Hiểu mẫu `Class::method` và `->method`. Không chèn kết quả khi toàn bộ là `[text call]` (nó chỉ lặp lại rg).
5. **Driver generic:** tôn trọng phần sau `::` và phân biệt `->name(` / `::name(` với `name(` (hàm toàn cục). Chỉ in mỗi dòng một lần. Thêm tên hàm chứa bằng một bộ tách hàm đơn giản (PHP tokenizer có sẵn trong `php` CLI kể cả khi thiếu vendor), để `locate` chạy được.
6. **Nhận composer ở thư mục con** (`*/composer.json` có `vendor/autoload.php` cạnh nó) để chạy driver PHP trên Perfex/CodeIgniter.
7. **Laravel:** nối closure đối số lồng trong closure route (`->block(fn)`, `DB::transaction(fn)`, `Cache::remember(fn)`) về closure cha. Nối `static::saving/creating/...(fn)` trong `booted()` thành `event eloquent.<x>: Model`. Đưa `schedule` vào lối vào. Giữ kiểu `Builder<Model>` qua scope để `Post::query()->published()` thành exact. Suy kiểu property từ phép gán `$this->p = $param` trong constructor.
8. **Hỏi tên mơ hồ hàm/class:** khi tên khớp cả hàm toàn cục lẫn class, báo "matches several symbols" như với `handle`.
9. Chỉ in cảnh báo vite khi có truy vấn JS/TS. Không in "raise --depth" khi người dùng đặt `--depth` rõ ràng.
