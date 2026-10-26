# Test codetrace vòng 2 trên PHP/Laravel (backend) — 25/10/2026 20:41

Repo: `repos/easy-ai-platform-backend` (`develop` @ `22bfbcbc8a`, chỉ đọc). Gọi CLI `codetrace callers '<đích>' --compact` (thêm `--depth 1` khi cần) và `codetrace locate`. Đối chiếu độc lập bằng `rg` + đọc code. Một repo PHP tí hon trong scratchpad (ngoài backend) dùng để tái hiện tối thiểu lỗi 1.

## Tóm tắt

- **Bỏ sót im lặng ở 7 dạng code khác nhau** (không có ở exact/likely/unresolved): method của trait (gọi qua tên trait hoặc `file:dòng`), `$x = $c ? $obj->m() : null; $x->n()`, `$class::m()` với class trong biến, `$this->$method()` (trái README), `Artisan::call('lệnh')`, `app(X::class)()` (gọi `__invoke`), method trong anonymous class.
- Lỗi nặng nhất là **trait**: `locate` trả về `Trait::m`, nhưng `callers` trên đúng symbol đó chỉ thấy lời gọi nội bộ trong trait; lời gọi thật từ các class dùng trait nằm dưới `UsingClass::m`, và hai bên không nối với nhau. Ví dụ `SensitiveStore::hydrate` (đọc dữ liệu nhạy cảm của 5 model) chỉ có 3 caller `likely` + 3 `unresolved`, trong khi thực tế mọi `->get()`, `->refresh()`, `->fresh()` trên `ChatUser`, `ChatConversation`, `ChatMessage`, `Customer`, `CustomerIdentity` đều đi qua.
- Không thấy `exact` sai. Có một `unresolved` gán nhầm class (lỗi 2, tái hiện ở repo tí hon).
- Interface nhiều bản cài đặt, `static::` trong class trừu tượng, observer, listener auto-discover, job (`dispatch(new J)`, `J::dispatch`, `Bus::chain`), callable `[$this, 'm']`, closure route: **đúng**.
- Thời gian ấm: 8–65 ms; lần đầu của một số đích 120–130 ms. Nhưng một truy vấn không giới hạn `--depth` (`ClassifyDirtyConversationsJob::handle`) làm **daemon deadlock**: lệnh đó treo khoảng 1.000 s và truy vấn `--depth 1` chạy sau treo hơn 710 s, không báo gì, cho tới khi có người khởi động lại daemon (lỗi 9).

## Kết quả theo đích

| # | Đích (dạng khó) | Chỗ gọi thật (rg + đọc tay) | Theo codetrace | Bỏ sót | Sai | Thời gian ấm | Cỡ output |
|---|---|---|---|---|---|---|---|
| 1 | `Commerce\Outbound\NhanhOutbound::pushInventoryLevel` — override method của class trừu tượng `Outbound`, lấy qua `Registry` (10 driver) | `Outbound.php:175` `$this->pushInventoryLevel` ← `pushInventoryOut` ← `SyncInventoryOutboundJob.php:77`, `InventoryLocationController.php:376` (`$outbound?->pushInventoryOut`) | `pushInventoryOut` `likely` ✓; job ✓; **thiếu `InventoryLocationController::updateStock` và route của nó** | 1 (lỗi 2) | 0 | 43 ms (lần đầu) / 9 ms | 1,4 KB |
| 1b | `Outbound::cancelInvoice`, `Outbound::cancelOrder`, `Inbound::applyWebhookEvent` (cùng dạng) | `OrderCancellation.php:68`, `:83`; `ProcessCommerceWebhook.php:67,70,72` | 0 caller cho `cancelInvoice`; `cancelOrder` chỉ có `ProcessOrderJob.php:182`; `applyWebhookEvent` chỉ có 7 `parent::` | 5 dòng (lỗi 2) | 0 | 21–128 ms | 0,5 KB |
| 2 | `Traits\SanitizesAiResponse::sanitizeAiResponse` / `::stripMarkup` — trait dùng ở 16 class | 7 dòng `$this->sanitizeAiResponse` (Chatbot, 4 transformer, AiMessageComposer); 5 dòng `self::stripMarkup` | **0 / 0** | 12 (lỗi 1) | 0 | 10 ms | 0,45 KB |
| 2b | `Chatbot::staticSanitizeAiResponse` (cùng trait, gọi theo tên class dùng) | `SanitizesAiResponse.php:59` (`self::` trong trait, chạy với `$this` là Chatbot) | 0 | 1 (lỗi 1) | 0 | 11 ms | — |
| 2c | `CustomerDataStorage\SensitiveStore::hydrate` (gọi trong trait `HydratesFromCustomerStorage`) | `:24` (anonymous Builder `get`), `:51` `refresh`, `:60` `fresh`; trên đó: mọi `->get()/refresh()/fresh()` của 5 model, ví dụ `ChatUserObserver.php:67` `$chatUser->refresh()` (`ChatUser $chatUser`) | 3 `likely` + 3 `unresolved` ở `ChatConversation.php`; không có `ChatUserObserver.php:67` | rất nhiều (lỗi 1, 7) | 0 | 36 ms | 1,4 KB |
| 3 | `Order\Providers\NhanhOrderProvider::getProviderIdentifier` — `static::` trong `AbstractOrderProvider` | `AbstractOrderProvider.php:43` ← `forBot` ← `OrderProviderFactory.php:17` (`$class::forBot`) | `:43` `[late static, likely]` ✓, `forBot` ✓, **dừng ở đó** | `OrderProviderFactory.php:17` (lỗi 3) | 0 | 17 ms | 0,9 KB |
| 3b | `Commerce\Outbound\NhanhOutbound::forBot` (abstract static) | `Registry.php:240` `$class::forBot($bot)` | 0 | 1 (lỗi 3) | 0 | 20 ms | 0,5 KB |
| 4a | `Listeners\HandoffSideEffects::handle` — listener auto-discover; event phát bằng `event(new ConversationHandedOff)` | `TurnPipeline.php:642,1106,1171`, `AnalyzeSentimentOnAgentTurn.php:100`, `HandoffOnWholesaleOrder.php:49` | `[event]` + đủ 5 nơi phát ở tầng trên; 14 entry point | 0 | 0 | 10 ms | 8 KB |
| 4b | `Observers\ChatUserObserver::created` — `ChatUser::observe(...)` ở `AppServiceProvider.php:444` | event `eloquent.created` | `event eloquent.created: App\Models\ChatUser` | 0 | 0 | 17 ms | 0,6 KB |
| 5 | `Jobs\RetrainDataSourceBatchJob::handle` — `J::dispatch`, `dispatchSync`, `new J` trong `Bus::chain` | 9 nơi (DataSourceController:1722, InsertUrlService:328/341/373, CrawlDispatcher:64, 4 command) | đủ cả 9; 34 entry point | 0 | 0 | 123 ms lần đầu / 11 ms | 10 KB |
| 5b | `Jobs\ClassifyDirtyConversationsJob::handle` — `dispatch(new J)` và `J::dispatch` | `AutoAssignService.php:181`, `AgentReportDirtyService.php:101,320`, `MetricsPipelineDispatcher.php:56` | `--depth 3`: đủ 4 | 0 | 0 | `--depth 3`: nhanh; không `--depth`: treo khoảng 1.000 s, daemon deadlock, kết thúc bằng `server closed the connection without an answer` (lỗi 9) | 47 B |
| 6a | `Services\EmailInbox\Providers\GmailMailboxProvider::fetchNew` — interface `MailboxProvider` 3 bản cài đặt, chọn bằng `MailboxProviders::for()` | `MailboxSyncer.php:31` ← job ← 3 controller + command | đủ, `likely` | 0 | 0 | 31 ms | 2 KB |
| 6b | `Listeners\DisconnectPeDatabase::__invoke` | `AppServiceProvider.php:376,377` `app(DisconnectPeDatabase::class)()`; `config/octane.php:102` | chỉ Octane `RequestTerminated` | 2 (lỗi 5) | 0 | 15 ms | 0,5 KB |
| 6c | `app(X::class)->m()` (vd. `AiAgentOnboardingService::shouldResumeOnboarding`, `Registry::outboundFor`) | — | luôn gắn `likely` dù X là class cụ thể, không bind | 0 | nhãn yếu hơn cần thiết (lưu ý) | 25 ms | 1,8 KB |
| 7a | closure route `routes/web.php:215` gọi `shouldResumeOnboarding` | `routes/web.php:223` + 3 dòng `LoginController` | `routes/web.php [call]` ✓; **entry point không có `GET /`** | entry point (lỗi 8) | 0 | 25 ms | 1,8 KB |
| 7b | `Jobs\CleanupExpiredPresence::handle` — `Schedule::job(new X)` (`routes/console.php:87`) | `routes/console.php:87` | `routes/console.php (file):1`; **"Affected entry points" trống** | entry point (lỗi 8) | 0 | 51 ms | 0,9 KB |
| 7c | `Console\Commands\DataSourceRetrain::handle` (`datasource:retrain`) | `WebApi/DataSourceController.php:709` `Artisan::call('datasource:retrain', …)` | chỉ `artisan datasource:retrain` | 1 (lỗi 4) | 0 | 11 ms | 0,6 KB |
| 7d | `Console\Commands\DataSourceRerank::handle` (`datasources:rerank`) | `CrawlerController.php:41`, `CrawlerCallbackController.php:47` | chỉ `artisan datasources:rerank` | 2 (lỗi 4) | 0 | — | — |
| 8a | `FunctionTools\Implementations\ViettelStoreFunction::checkStoreStock` — chỉ được gọi bằng `$this->$methodName(...)` | `BotFunction.php:160` | 0 (cả `unresolved` cũng không) | 1 (lỗi 6) | 0 | 10 ms | 0,5 KB |
| 8b | `DiDongVietFunction::getPromotionAttribute`, `BaseBotFunction::getPriceAttribute` | `BaseBotFunction.php:818` `$this->$methodName($product)` | 0 / 0 | 2 (lỗi 6) | 0 | 19 ms | — |
| 8c | callable `[$this, 'm']`: `CardFormatter::mdCell`, `BrowserAuthorizationService::normalizeUri`, `TopGiftExtractor::productLine`, `BaseBotFunctionV3::toShortLink` (`Closure::fromCallable`) | `CardFormatter.php:360,368`; `BrowserAuthorizationService.php:184,191`; `TopGiftExtractor.php:81`; `BaseBotFunctionV3.php:58` + `ViettelStoreFunction.php:260` | đủ, nhãn `[callable]` | 0 | 0 | 9–19 ms | `toShortLink` toàn cây: 57 KB, 687 symbol, 87 entry point |
| phụ | `ShopifyInbound::statusFromRaw` → method trong anonymous class | `ShopifyInbound.php:559` trong `new class implements StatusMapperInterface { fromRaw }` ← `Inbound.php:3148` `$mapper?->fromRaw` | `App\Services\Commerce\Inbound\fromRaw` rồi dừng | chuỗi bị cắt (lỗi 7) | tên symbol sai | 10 ms | 0,6 KB |

## `locate` khó

| Dòng | Nằm ở | codetrace trả | Đánh giá |
|---|---|---|---|
| `app/Services/BusinessObject/Writing/DealWriter.php:313` | arrow fn lồng trong arrow fn (`->each(fn … => withoutEvents(fn() => $item->delete()))`) trong closure `DB::transaction` | `DealWriter::delete :307` | đúng (method bao ngoài) |
| `app/Services/Commerce/Registry.php:197` | `static fn(Outbound $o) => $o->name()` trong `array_map` | `Registry::outboundNamesSupporting :193` | đúng |
| `app/Services/Commerce/Inbound/NhanhInbound.php:120` | method `fromRaw` của anonymous class trong `statusMapper()` | `App\Services\Commerce\Inbound\fromRaw :119` | **sai**: trông như hàm cấp namespace; trùng tên với `fromRaw` ở `ShopifyInbound.php:557` (cùng chuỗi `App\Services\Commerce\Inbound\fromRaw`) |
| `app/Models/Concerns/HydratesFromCustomerStorage.php:24` | method `get` của anonymous `Builder` trong trait | `App\Models\Concerns\get :22` | **sai**, cùng kiểu |
| `app/Models/Concerns/HydratesFromCustomerStorage.php:26` | dòng đóng `};` của anonymous class | `HydratesFromCustomerStorage::newEloquentBuilder :19` | đúng |

Mỗi lần 8–9 ms.

## Lỗi tìm được

### 1. Trait: lời gọi bị chia làm hai đồ thị không nối nhau (bỏ sót im lặng, nặng)

- `codetrace locate app/Traits/SanitizesAiResponse.php:58` → `App\Traits\SanitizesAiResponse::sanitizeAiResponse`. `callers` trên symbol này (hoặc `callers app/Traits/SanitizesAiResponse.php:57`) → **0 caller**. Thực tế 7 dòng: `Chatbot.php:2680`, `WebResponseTransformer.php:287,375`, `ZaloResponseTransformer.php:72`, `MessengerResponseTransformer.php:82`, `InstagramResponseTransformer.php:88`, `AiMessageComposer.php:373`.
- `App\Traits\SanitizesAiResponse::stripMarkup` → 0. Thực tế: `AiMessageComposer.php:333,359,376`, `WorkflowAiLabelService.php:329`, `InteractExternalCommentAction.php:508`. Hỏi `App\Services\AiMessageComposer::stripMarkup` thì ra 3 dòng của riêng class đó.
- Ngược lại, lời gọi **trong thân trait** chỉ gắn với tên trait: `App\Services\Chat\Chatbot::staticSanitizeAiResponse` → 0, dù `SanitizesAiResponse.php:59` gọi `self::staticSanitizeAiResponse` khi chạy trong Chatbot. `App\Traits\SanitizesAiResponse::staticSanitizeAiResponse` → 1 caller (`:59`) rồi **dừng**, không đi tiếp lên `Chatbot.php:2680` hay route nào.
- Cùng kiểu với `static::` qua trait: `App\Traits\ExtractScriptData::extractScriptData` → 0; `App\Models\DataSource::extractScriptData` → `DataSource.php:1013`.
- Cùng kiểu, hậu quả lớn nhất: `SensitiveStore::hydrate` ← `HydratesFromCustomerStorage::refresh/fresh/get`; tầng trên chỉ ra 3 dòng `unresolved` ở `ChatConversation.php:2386,2989,3131`. Trong khi đó `App\Models\ChatUser::refresh` (cùng body `HydratesFromCustomerStorage.php:48`) có 5 exact + 25 unresolved, ví dụ `ChatUserObserver.php:67` `$chatUser->refresh()` với `ChatUser $chatUser`. Agent hỏi "ai chạm tới dữ liệu nhạy cảm" sẽ nhận một danh sách gần như trống.

Cần: `callers Trait::m` = hợp các `UsingClass::m` (và ngược lại, cạnh trong thân trait phải được nhân ra cho từng class dùng trait).

### 2. `$x = $cond ? $obj->m() : null; $x->n()` mất dấu, có khi gán sang nhầm class (bỏ sót im lặng + `unresolved` vô lý)

Bằng chứng trong backend (đều không có ở exact/likely/unresolved):
- `InventoryLocationController.php:373-376`: `$outbound = $bot ? $this->registry->outboundFor(...) : null; $outbound?->pushInventoryOut(...)`, với `outboundFor(): ?Outbound`. `callers Outbound::pushInventoryOut` chỉ ra `SyncInventoryOutboundJob.php:77`.
- `OrderCancellation.php:61,68,83`: `$outbound = $bot ? $this->providers->outboundFor(...) : null;` rồi `$outbound->cancelInvoice(...)`, `$outbound->cancelOrder(...)`. `Outbound::cancelInvoice` → 0 caller.
- `ProcessCommerceWebhook.php:56,67,70,72`: `$inbound = $bot ? $registry->inboundFor(...) : null;` rồi `acceptsWebhook`, `parseWebhook`, `applyWebhookEvent` → không có ở `Inbound::*` lẫn `ShopifyInbound::*`.

Tái hiện ở repo tí hon (scratchpad): `Reg::find(): ?Base`, `abstract class Base { ping() }`, `class Child extends Base`.
- `$x = $f ? $this->reg->find('n') : null; $x->ping();` → `Base::ping` 0, `Child::ping` 0. Thêm `Reg::ping()` thì dòng đó hiện thành `unresolved` của **`Reg::ping`**: gán theo kiểu của receiver ở nhánh then (`Reg`), không theo kiểu trả về (`?Base`).
- Đối chứng: `$x = $this->reg->find('n'); $x->pong();` → `Base::pong` exact, `Child::pong` likely (đúng). `$box = $f ? $this->make() : null; $box->open();` → `unresolved` (chấp nhận được). `$f ? $this->make() : new Box()` → exact.
- Riêng `Registry.php:169-170` (`$sibling ? $this->outboundFor(...) : null`, receiver là `$this`) thì ra `unresolved`, không mất.

### 3. Gọi static với class trong biến `$class::m()` bị bỏ qua hoàn toàn (bỏ sót im lặng)

- `Registry.php:240` `return $class::forBot($bot);`: `NhanhOutbound::forBot` và `Outbound::forBot` → 0 caller, kể cả `unresolved`.
- `OrderProviderFactory.php:17` `$instance = $class::forBot($bot);`: chuỗi `NhanhOrderProvider::getProviderIdentifier` ← `AbstractOrderProvider::forBot` dừng ở đó, không tới `OrderStateBuilder.php:79`, trait `InteractsWithOrder.php:54`.
- `InventoryProviderFactory.php:55` `$class::forBot($bot)`: `AbstractInventoryProvider::forBot` → 0.
- `Inbound.php:3146` `$inboundClass::statusMapper()`: `ShopifyInbound::statusMapper` → 0, `Inbound::statusMapper` → 0.

Đây đúng là dạng gọi động mà README hứa luôn liệt kê `unresolved`.

### 4. `Artisan::call('tên-lệnh')` không nối tới command (bỏ sót im lặng)

- `WebApi/DataSourceController.php:709` `Artisan::call('datasource:retrain', …)` → `DataSourceRetrain::handle` chỉ có `artisan datasource:retrain`.
- `CrawlerController.php:41`, `CrawlerCallbackController.php:47` `Artisan::call('datasources:rerank', …)` → `DataSourceRerank::handle` không có.
- `CrawlerCallbackController.php:48` `Artisan::call('bot:retrain-all', …)`; `SeedEcommerceDemoCommand.php:102` `$this->call('bo:seed-types')`: `RetrainAllBots::handle` và `BusinessObject\SeedTypesCommand::handle` chỉ có `artisan …`, thiếu hai dòng này.

Tool đã biết ánh xạ signature → class (vì in được `artisan datasource:retrain`), nên chỉ cần nối chuỗi literal trong `Artisan::call`/`$this->call`.

### 5. Gọi trực tiếp object lấy từ container `app(X::class)()` không nối tới `__invoke` (bỏ sót im lặng)

`AppServiceProvider.php:376-377` `Queue::after(fn() => app(DisconnectPeDatabase::class)());` → `DisconnectPeDatabase::__invoke` chỉ có Octane `RequestTerminated` (`config/octane.php:102`), thiếu hai hook queue.

### 6. `$this->$method()` không được liệt kê `unresolved` (trái README:78)

README: gọi qua tên lấy từ biến "luôn được liệt kê `unresolved`". Thực tế:
- `BotFunction.php:160` `$this->$methodName($args, $chatConversationId)` (cách mọi tool AI được gọi): `ViettelStoreFunction::checkStoreStock` (`ViettelStoreFunction` → `BaseBotFunctionV3` → `BotFunction`) → 0 caller.
- `BaseBotFunction.php:818` `$this->$methodName($product)`: `DiDongVietFunction::getPromotionAttribute`, `BaseBotFunction::getPriceAttribute` → 0.

Cần ít nhất: mọi `$this->$x(...)` trong class cha/chính class đó là `unresolved` cho mọi method của class con.

### 7. Method trong anonymous class: tên symbol sai và chuỗi bị cắt

- `locate NhanhInbound.php:120` → `App\Services\Commerce\Inbound\fromRaw`; `locate ShopifyInbound.php:559` → **cùng tên đó** (5 file trong namespace có anonymous `fromRaw`: Nhanh, Shopify, Beesky, KiotViet, Haravan). Symbol bị trùng, `callers` theo tên không phân biệt được.
- `callers app/Services/Commerce/Inbound/NhanhInbound.php:120` → 0, không có `unresolved`. Lời gọi thật: `Inbound.php:3148` `$mapper?->fromRaw($raw)` (`$mapper` là `StatusMapperInterface`); tool có thấy dòng này khi hỏi `StatusMapperInterface::fromRaw`, nhưng không nối anonymous class (implements interface đó) vào.
- `HydratesFromCustomerStorage.php:22-24` (anonymous `extends Builder`, override `get`) → `App\Models\Concerns\get`; caller của nó là mọi truy vấn trên 5 model, tool trả 0.

### 8. Entry point bị mất với closure route và scheduler

- `routes/web.php:215` `Route::get('/', function (Request $r, AiAgentOnboardingService $onboarding) {…})`: caller `routes/web.php:223` có, nhưng "Affected entry points" của `shouldResumeOnboarding` không có `route GET /`.
- `CleanupExpiredPresence::handle`: lên tới `routes/console.php (file):1` (do `Schedule::job(new CleanupExpiredPresence)` `:87`), nhưng "Affected entry points" **trống**. Agent tưởng job không có lối vào.
- `SyncEmailMailboxesCommand::handle`: chỉ `artisan email:sync`, không nói `Schedule::command('email:sync')->everyMinute()` (`routes/console.php:36`). Đây là thiếu thông tin, không phải sót caller.

### 9. Daemon treo cứng sau một truy vấn không giới hạn `--depth`, chặn mọi truy vấn sau (nặng)

Diễn biến (khoảng 20:57–21:15):
1. `codetrace callers 'App\Jobs\ClassifyDirtyConversationsJob::handle' --compact` (không `--depth`). Trước đó cùng đích với `--depth 3` trả ngay, và 15 đích khác trong cùng vòng chạy trả trong 9–130 ms.
2. Lệnh này không bao giờ trả về: lúc viết báo cáo client (pid 1483326) đã chờ **hơn 940 s**, không in gì, không timeout, không báo lỗi.
3. Một truy vấn nhỏ sau đó (`callers 'App\Services\Commerce\Inbound\ShopifyInbound::statusMapper' --depth 1`, pid 1486986) cũng treo **hơn 710 s**. `ss -xp` cho thấy 2.048 byte yêu cầu nằm trong hàng đợi gửi, daemon không đọc socket đó (`Recv-Q 253` ở phía `serve`).
4. Trạng thái tiến trình: `codetrace serve …/easy-ai-platform-backend` (pid 1461290) 14 thread, 10 thread chờ trong `anon_pipe_read`, CPU 0,8%. Driver PHP `driver.php` (pid 1461301) và 6 tiến trình con của nó (1479391, 1479407, 1479595, 1479611, 1483280, 1483343; hai cái cuối sinh đúng lúc lệnh 1 chạy) đều ngủ trong `poll`, CPU 0%. Không bên nào làm việc: **deadlock giữa daemon và driver PHP**, không có watchdog. Máy lúc đó hết swap (4.088/4.095 MB), nhưng cây tiến trình còn nguyên (daemon, driver, 6 tiến trình con), không thấy tiến trình nào mất.
5. Trong suốt thời gian treo, client không in `Waited:`, tiến độ hay gì khác; agent không phân biệt được "đang tính" với "đã chết".

6. Khoảng 21:15, ai đó ngoài phiên này khởi động lại daemon (pid mới 1527542). Lúc đó lệnh 1 mới kết thúc, sau khoảng 1.000 s, với một dòng duy nhất `server closed the connection without an answer`. Lệnh 3 được trả lời ngay, và daemon mới trả lời các truy vấn sau trong 17–76 ms.

Vòng 1 ghi "đã sửa treo daemon"; lỗi vẫn còn ở dạng khác. Hậu quả: mọi agent hỏi repo backend đều bị treo cho tới khi có người kill daemon. Tôi không kill `codetrace serve`. Chưa xác định được nguyên nhân deadlock, cũng chưa biết truy vấn này có xong được nếu không bị chặn hay không.

### 10. Nhiễu / nhỏ

- Mọi output in `Warning: tests/Feature/CRM/BusinessObjectPaymentTest.php could not be analysed (PHP aborted while analysing it)`, kể cả khi không hỏi `--tests`: nhiễu lặp lại; và là một file thật không phân tích được.
- Mỗi job/event sinh 5–7 symbol giả (`::dispatch`, `dispatchIf`, `dispatchUnless`, `dispatchSync`, `dispatchAfterResponse`, `broadcast`) trong "Further up" → output phình (đích 4a: 8 KB, phần lớn là các dòng này). Chúng còn ăn 1 tầng `--depth`: `--depth 1` trên `Job::handle` chỉ ra `__construct [queue]`, cần `--depth 3` mới thấy nơi dispatch thật.
- Ẩn test nhưng vẫn lộ một dòng test trong mục unresolved: đích 4a in `tests/Feature/Agent/Concerns/EnablesAgentHarness.php:46`.
- `app(X::class)->m()` luôn là `likely` dù `X` là class cụ thể không bind (vd. `LoginController.php:267`); có thể là `exact`.
- Gọi qua thuộc tính quan hệ Eloquent (`$node->workflow->incrementTotals`, 22 dòng) là `unresolved`; vẫn liệt kê, không sót.
- `Closure::fromCallable([$this,'toShortLink'])` được tính là `__construct` gọi `toShortLink` (thực tế closure chạy sau, ở `CardFormatter.php:426`) → vùng ảnh hưởng 687 symbol / 87 entry point, 57 KB output.
- Dòng `Cost:` báo 950 MB – 1,3 GB (proportional). Đo bằng `ps` lúc 21:13: driver PHP 1461301 có RSS 679 MB, cộng 6 tiến trình con mỗi cái 608–615 MB RSS. Bốn con trong số đó (1479391, 1479407, 1479595, 1479611) sinh khoảng 6 phút trước deadlock, từ những truy vấn đã trả lời xong, và vẫn sống idle hơn 20 phút. Tổng RSS thực tế của cây tiến trình lớn hơn nhiều so với con số in ra (RSS tính cả bộ nhớ dùng chung, nên không cộng thẳng được).

## Đúng, không có lỗi

- Interface 3 bản cài đặt lấy từ registry (`MailboxProvider::fetchNew`): đủ chuỗi tới 3 route + `artisan email:sync`.
- `static::getProviderIdentifier()` trong class trừu tượng → override ở class con: `likely`, đúng.
- Listener auto-discover (`HandoffSideEffects`), event phát bằng `event(new X)`: đủ 5 nơi phát.
- Observer đăng ký bằng `Model::observe` → `event eloquent.created`.
- Job: `J::dispatch`, `dispatchSync`, `dispatch(new J)`, `new J` trong `Bus::chain`: đủ.
- Callable `[$this,'m']` trong `array_map`, `Closure::fromCallable`: đủ.
- Đăng ký listener trong `config/octane.php`: nhận ra.
