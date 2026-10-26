# Test luồng sửa lỗi: codetrace (A) và rg + đọc file (B)

Ngày 25/10/2026, repo `repos/easy-ai-platform-backend` (develop, chỉ đọc). Gọi codetrace qua MCP (`locate`, `callers`, bản gọn mặc định); vài lần đo lại bằng CLI `codetrace` (cùng binary, cùng daemon). Token ước theo byte / 4.

## Kết luận nhanh

- **Người gọi trực tiếp:** A và B ra giống nhau ở cả hai tình huống. A thêm được thứ B không làm nổi bằng tay: cây đi ngược tới 37 route/lệnh (PHP) và 53 trang (JS).
- **Sót:** A bỏ sót đúng chỗ quan trọng nhất của tình huống JS: `InputSearchSelect.vue`, nơi gọi hàm thật và đọc shape trả về. Chỗ này không nằm ở `exact` mà cũng không ở `unresolved`, tức là bị bỏ im lặng. Ở PHP, A không chỉ ra các chỗ `catch` exception và chỗ lặp cứng cache key. Hai thứ này nằm ngoài khái niệm "caller", nhưng người sửa vẫn phải xem.
- **Tốc độ:** khi daemon đã nóng, mỗi lần hỏi 11–46 ms, đạt mục tiêu. Gọi qua MCP thì không ổn định: 2 lần mất 18 s, có 1 lần **treo 1800 s** rồi client tự cắt, không có thông báo lỗi nào. Dòng `Cost:` có lần ghi 46 ms trong khi thời gian thật là 59 s.
- **Context:** B tốn ít hơn (PHP 6,5 KB so với 20 KB; JS 5,3 KB so với 10 KB). Phần thừa của A chủ yếu là nhiễu, nói ở mục 4.

## 1. Hai tình huống

| | Dòng xuất phát | Giả sử sửa |
| --- | --- | --- |
| PHP | `app/Services/ChatUserBlockService.php:42` (nằm trong closure `Cache::remember` của `isBlocked`) | đổi chữ ký/hành vi `isBlocked(?int $botId, string $guestId)` |
| JS | `resources/js/composables/useCdpFormatters.js:407` (`label: customer.display_name ...`) | đổi shape object do `customerToSearchOption` trả về |

## 2. Cách A: codetrace

### PHP

| Lần gọi | Công cụ | `Cost:` | Ghi chú |
| --- | --- | --- | --- |
| 1 | `locate` `...ChatUserBlockService.php:42` | 1 347 ms | Ra đúng `ChatUserBlockService::isBlocked` (`:35#e6c0`), nhận ra closure thuộc method |
| 2 | `callers` `...::isBlocked` | 18 195 ms | Daemon rút dữ kiện lần đầu |
| 3 | `callers` `...ChatUserBlockService.php:42` | không có | **Treo 1800 s**, Claude Code tự cắt |
| 4 | `callers` `...ChatUserBlockService.php:42` | 17 998 ms (~53 s CPU) | Daemon lại rút từ đầu |
| CLI ×3 | `callers --compact` | 46 / 18 / 40 ms | Lần đầu đo ngoài **59,31 s**, sau đó 0,03 và 0,05 s |

Output gọn 19 982 B (~5k token). Bản `--format full` 86 006 B (~21k token).

Danh sách cần xem lại theo A:
- Người gọi trực tiếp (1): `app/Services/Chat/Chatbot.php:2240` `Chatbot::assertNotBlocked` (gắn `likely`).
- Đi lên: 245 symbol trong 53 file, 37 điểm vào (route webhook của Messenger, Zalo, WhatsApp, Instagram, Telegram, Shopee, TikTok; `/api/chat`; sandbox; sync; 3 lệnh artisan). Bản gọn không có số dòng của tầng này. Phải lấy `--format full` mới thấy `Chatbot.php:589` (`ingestInboundOnly`), `:633` (`getResponse`), `:701` (`setupTextStream`).
- `unresolved` (2): `app/Models/WorkflowNode.php:1874`, `tests/Feature/Agent/Concerns/EnablesAgentHarness.php:46`.
- Ẩn: 142 caller nằm trong test.

### JS

| Lần gọi | Công cụ | `Cost:` |
| --- | --- | --- |
| 1 | `locate` `...useCdpFormatters.js:407` | 24 ms, ra `useCdpFormatters.js#customerToSearchOption` (`:395#ee99`) |
| 2 | `callers` symbol đó | 20 ms |
| CLI | bản full / compact / compact `--tests` | 28 / 11 / 11 ms (đo ngoài 0,04 s) |

Output gọn 10 322 B (~2,6k token). Bản full 79 009 B (~20k token). Daemon đã nóng từ PHP nên chưa đo riêng được lần đầu của driver JS.

Danh sách cần xem lại theo A:
- Người gọi trực tiếp (2): `resources/js/Components/CDP/CdpCreateDealSheet.vue:132` (`handleCustomerCreated`) và `useCdpFormatters.js:418` (`.map(customerToSearchOption)` trong `mapCustomerSearchResponse`).
- Đi lên: 85 symbol trong 78 file, gồm `CdpDealFormPanel.vue#customerField`, `CdpCompanyPeoplePanel.vue#personField`, `CdpCompanyTimeline.vue#taskFields`, `Pages/cdp/tasks/show.vue#detailRows`, rồi lan qua `data-field/DataFieldInput.vue#COMPONENTS` và `data-grid/*` tới mọi trang ecommerce, zma, call-center.
- 53 điểm vào (trang nạp ở `resources/js/app.js:26`).
- `unresolved` 132 dòng, tất cả quy về `resources/js/ssr_unused.js#resolve`.

## 3. Cách B: rg + đọc file

Thời gian dưới đây là lúc chạy lại đúng các lệnh đó và đo bằng `date +%s%N`. Không tính thời gian suy nghĩ giữa các lệnh.

| | Số lệnh | Thời gian chạy | Byte đọc |
| --- | --- | --- | --- |
| PHP | 4 | 124 ms | 6 541 B (~1,6k token) |
| JS | 3 | 50 ms | 5 349 B (~1,3k token) |

PHP, những gì B thấy:
- Người gọi trực tiếp: chỉ `Chatbot.php:2240` (`rg 'isBlocked\b'`; các kết quả còn lại là biến trùng tên trong Vue). Service được nạp ở `Chatbot.php:2236` qua `app(ChatUserBlockService::class)`. `SecurityChatUserBlockController.php:16` có inject service nhưng không gọi `isBlocked`.
- Tầng trên: `assertNotBlocked` được gọi ở `Chatbot.php:589`, `:633`, `:701`.
- Nơi bắt `ChatUserBlockedException` (ném ở `Chatbot.php:2248`): `WhatsappController.php:1196`, `ZaloController.php:1914`, `:2172`, `MessengerController.php:3639`, `:4004`, `:4388`, `InstagramController.php:2058`, `Listeners/ProcessChatMessage.php:218`, `Listeners/ProcessChatMessageStream.php:250`.
- Cache key dính với nhau: `cacheKey()` ở `ChatUserBlockService.php:30-33`. `blockManual` xoá cache bằng `cacheKey` (`:97`), còn `unblock` **lặp cứng** chuỗi `"chat_block:{$row->bot_id}:{$row->guest_id}"` (`:121`). Đổi định dạng key hoặc tham số của `isBlocked` thì phải sửa cả dòng 121.
- B không dựng được cây 37 điểm vào. Muốn có thì phải `rg` lặp hơn 8 tầng qua 245 symbol, làm tay không thực tế.

JS, những gì B thấy:
- `useCdpFormatters.js:418` và `CdpCreateDealSheet.vue:58,132`.
- `mapCustomerSearchResponse` được truyền dưới dạng `mapResponse` ở `Pages/cdp/tasks/show.vue:307,478` (qua `searchField`, `:430-440`), `CdpDealFormPanel.vue:655,2084`, `Company/CdpCompanyTimeline.vue:409,580`, `Company/CdpCompanyPeoplePanel.vue:100,129`.
- **Nơi gọi thật và đọc shape:** `resources/js/Components/Form/Input/InputSearchSelect.vue:166-167` (`props.field.mapResponse(payload)`). Shape được đọc ở `:125-143` (`valueKey` mặc định `value`, `labelKey` mặc định `label`, `option.description`) và render ở `:60-76`.

## 4. So sánh

### Cách nào tìm đủ hơn

A thấy mà B (làm tay hợp lý) không thấy:
- Cây đầy đủ tới 37 route/lệnh (PHP) và 53 trang (JS), dùng để chọn luồng cần thử lại.
- Số dòng các tầng gián tiếp, nhưng chỉ có ở `--format full`.

B thấy mà A sót:
1. **JS: `InputSearchSelect.vue:166-167` và `:125-143`.** Đây là nơi duy nhất thực sự gọi `mapCustomerSearchResponse`, tức là gọi `customerToSearchOption`, và đọc `value`/`label`/`description` của kết quả. Đổi shape thì vỡ ở đây. Chạy `grep -c InputSearchSelect` trên bản full ra `0`. Chỗ này không có ở `exact` và cũng không ở `unresolved`, nên là bỏ im lặng, trái với cam kết "không bỏ im lặng" trong `plan.md`. Nguyên nhân: driver nối hàm truyền trong object (`mapResponse: mapCustomerSearchResponse`) về nơi **dựng** object, không về nơi **gọi** `props.field.mapResponse(...)`.
2. PHP: 9 chỗ `catch (ChatUserBlockedException)` liệt kê ở mục 3. Cây A có các controller này, nhưng chỉ vì chúng gọi `Chatbot`. Nhìn cây không biết chúng xử lý hệ quả của `isBlocked`. Đây là phụ thuộc kiểu exception, nằm ngoài khái niệm caller, nhưng vẫn là chỗ phải xem lại.
3. PHP: cache key lặp cứng ở `ChatUserBlockService.php:121`. Đây là phụ thuộc dữ liệu, codetrace theo thiết kế không thấy.

### Cách nào nhanh, ít context hơn

- Khi daemon nóng thì thời gian tương đương: A 11–46 ms một lần hỏi, B 6–52 ms một lệnh. Thời gian thật bị chi phối bởi số lượt agent, và A ít lượt hơn (2 so với 3–4).
- Khi daemon nguội hoặc bị thay, A thua hẳn: 18 s, 18 s, 59 s, và một lần treo 30 phút. Trong lúc phiên này chạy còn 2 phiên khác cùng dùng codetrace. `ps` ngay sau lần treo cho thấy daemon `serve` của backend (PID 1220722) mới chạy được 2 giây, do lệnh CLI của phiên khác khởi động. Daemon cũ đã mất trong lúc MCP đang chờ. Phiên này không xác định được ai hạ nó. Dù vậy, MCP **không báo lỗi khi daemon mất mà treo tới hết timeout**: đây là lỗi riêng, cần sửa.
- Context: B ít hơn ở cả hai tình huống (PHP ~1,6k so với ~5k token; JS ~1,3k so với ~2,6k). Nếu cần số dòng tầng gián tiếp thì A phải dùng bản full, ~20k token.
- RAM daemon và driver: 407 MB lúc `locate` đầu, lên 889–1 046 MB.

### Output codetrace gây hiểu nhầm hoặc thừa

1. **`Cost:` không phản ánh thời gian agent chờ.** CLI ghi `Cost: 46 ms` khi thời gian đo ngoài là 59,31 s. `Cost` chỉ đếm pha trả lời, bỏ qua lúc chờ daemon hoặc driver dựng lại.
2. **Nhóm file rỗng** `- : App\Jobs\AutoSyncChannelJob::__construct, ...::dispatch, ...dispatchIf, ...dispatchSync, ...dispatchAfterResponse, ...dispatchUnless` × 10 job, tổng khoảng 60 symbol ảo không có trong source. Nhóm này làm phồng con số "245 symbols".
3. `routes/api.php: routes/api.php` (×4): tên file lặp lại thành tên symbol.
4. `unresolved` dẫn sai: `app/Models/WorkflowNode.php:1874` là `(new ZnsTemplateService(...))->sendMessage(...)`. Kiểu của receiver biết được tĩnh, đây là class khác, không phải `ZaloController::sendMessage`.
5. `unresolved` thứ hai nằm trong `tests/...EnablesAgentHarness.php:46` dù test đang ẩn mặc định. Như vậy không nhất quán.
6. JS: 132 dòng `unresolved` đều là `path.resolve(...)` trong `.opencode/**` (công cụ, không phải code sản phẩm). Chúng khớp theo tên với thuộc tính `resolve` của `createInertiaApp` trong `resources/js/ssr_unused.js`. File này không được tham chiếu ở đâu (`rg ssr_unused` ra rỗng), vậy mà vẫn thành điểm vào và nguồn nhiễu. Nên loại các thư mục ẩn như `.opencode/` và file không thuộc Vite `input`.
7. JS: "Hidden: 6 callers inside tests" thật ra là 6 dòng `unresolved` `resolve` nữa. Thêm `--tests` chỉ làm 132 thành 138, không hiện caller test nào.
8. JS: cây nổ qua `DataFieldInput.vue#COMPONENTS` → `DataGrid*` ra 53 trang (ecommerce, zma, call-center). Về kỹ thuật thì đúng là "tới được", nhưng không giúp khoanh vùng. Vùng thật chỉ 5 file cộng `InputSearchSelect`.
9. Bản gọn bỏ số dòng của mọi caller gián tiếp. Đề xuất: giữ `path:line` cho mỗi caller, chỉ bỏ đoạn code.
10. Warning `tests/Feature/CRM/BusinessObjectPaymentTest.php could not be analysed` lặp ở mọi câu trả lời, kể cả `locate` của file JS.
11. Caller trực tiếp PHP gắn `likely`, trong khi `app(ChatUserBlockService::class)` quyết định được tĩnh. Lỗi nhỏ.

### Phát hiện phụ, ngoài phạm vi test

Trong `ChatUserBlockService`, `isBlocked` cache theo `chat_block:<bot>:<guest>`, và key này cũng chứa kết quả của chặn toàn cục (`whereIn` có `GLOBAL_BOT_ID`, `:42`). Khi `unblock` một dòng toàn cục (`:121`) hoặc `blockManual` toàn cục (`:97`), chỉ key `chat_block:<GLOBAL>:<guest>` bị xoá. Key theo từng bot vẫn giữ kết quả cũ tới hết `CACHE_TTL_SECONDS`. Cần xác nhận lại trước khi coi đây là lỗi.

## 5. Đề xuất cho codetrace

1. MCP phải trả lỗi ngay khi mất daemon hoặc socket, có timeout. Không được treo.
2. `Cost:` nên ghi cả thời gian chờ daemon sẵn sàng, hoặc ghi riêng "chờ dựng: X ms".
3. Nối hàm truyền qua thuộc tính object (`mapResponse: fn`) tới nơi gọi `x.mapResponse(...)`. Ít nhất phải liệt kê nơi đó là `unresolved`.
4. Bỏ symbol ảo `dispatch*` và nhóm file rỗng. Loại `.opencode/` và file không có ai nạp khỏi nguồn khớp theo tên.
5. Bản gọn giữ `path:line` cho caller gián tiếp.
6. Có thể thêm mục "nơi bắt exception mà đích ném ra", vì đây là phần vùng ảnh hưởng thường bị quên khi đổi hành vi.

Status: DONE_WITH_CONCERNS
Tóm tắt: codetrace tìm người gọi trực tiếp đúng như rg và dựng được cây điểm vào mà làm tay không nổi, nhưng bỏ sót im lặng `InputSearchSelect.vue` (nơi thật sự dùng kết quả ở tình huống JS), có nhiều nhiễu (symbol ảo, 132 dòng `path.resolve`), và qua MCP từng treo 30 phút; `Cost:` có lần báo 46 ms khi thời gian chờ thật là 59 s.
