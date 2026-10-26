# Test codetrace vòng 3 trên PHP/Laravel (backend): 25/10/2026 22:24

Repo thật: `repos/easy-ai-platform-backend` (`develop` @ `22bfbcbc8a`, chỉ đọc). Lệnh: `codetrace callers '<đích>' --depth 1 --compact` (không giới hạn depth khi test chuỗi sâu) và `codetrace locate`. Tôi tự kiểm sự thật bằng `rg` và đọc code. Thời gian ghi ở dạng "server ms / tổng CLI ms" (tổng CLI gồm cả wrapper `find` + cargo check, nên thường khoảng 60–90 ms).

Bản sao để sửa code: `scratchpad/php3/be` (đã xoá khi xong). **Lưu ý về cách dựng:** nếu symlink nguyên `vendor` như hướng dẫn thì `vendor/composer/autoload_psr4.php` lấy `__DIR__` đã resolve symlink, nên mọi class `App\` được nạp từ **repo thật** (`ReflectionClass(SubscriptionPlan)->getFileName()` ra đường dẫn backend thật). Sửa trong bản sao lúc đó có thể không có tác dụng. Tôi dựng lại theo cách sau: `vendor/` là thư mục thật, các package là symlink, riêng `vendor/composer`, `vendor/autoload.php` và `vendor/laravel` (33 MB) được copy thật. Sau đó 11 truy vấn mốc cho kết quả giống hệt repo thật (bỏ qua dòng `Cost` và cảnh báo typescript). Các package vendor khác vẫn nằm ngoài repoRoot của bản sao, nên suy luận kiểu qua chúng trên bản sao có thể kém hơn trên repo thật. Nếu vòng sau dùng lại cách dựng này thì cần sửa hướng dẫn.

## Tóm tắt

- **7 mục đã sửa từ vòng 2 đều chạy đúng:** trait (7 exact), `$x = $c ? $o->m() : null` (`OrderCancellation.php:68` exact), `$class::m()` (unresolved, có liệt kê), `$this->$m()` (unresolved), anonymous class (`NhanhInbound@anonymous_118::fromRaw`), `Artisan::call` (`[artisan]`), `app(X::class)()`. Entry point `route GET /` cũng đã có.
- **Lỗi nặng nhất vòng này: `Model::query()->…->find()/first()/findOrFail()` rồi gọi method thì bỏ sót im lặng.** Lỗi gặp trên repo thật (`SyncRunJob.php:43,59`, `AutoReplyController.php:19`) và tái hiện được trên bản sao. Đây là cách viết Eloquent phổ biến nhất trong backend.
- **Các method Laravel gọi theo quy ước đều ra 0 caller, không báo gì:** scope, accessor (cả kiểu cũ lẫn `Attribute`), middleware (route và global), form request `rules/authorize`, policy, `#[ObservedBy]`, `broadcastOn/With`, `__call/__callStatic`. Chỗ gọi macro (`->withSensitive()`, `->excludingCustomerStorage()`, `Str::ct3Slug()`) cũng mất hết, chỉ còn dòng đăng ký trong provider được tính là caller. Facade không nối về class thật.
- **Entry point sai:** route viết bằng arrow fn (`Route::get('/x', fn() => …)`, riêng `routes/web.php` có 177 route) không thành `route GET /x` mà thành "top-level code: runs when the file is loaded". Ngược lại, mọi `Route::…->group(function () {` bị tính thêm thành entry point giả, chiếm 21–27% danh sách entry point.
- **Hỏi một symbol không tồn tại** (gõ sai, đã xoá, đã đổi tên) thì không báo lỗi, rc=0, và tool trả về các dòng khớp chữ trong `.md/.csv/.json/.js.map`.
- **Treo:** deadlock của vòng 2 giờ bị watchdog cắt sau 20 s (`a driver restarted`), nhưng tình trạng treo vẫn còn: truy vấn sâu đầu tiên mất 23,9 s, lần hai mất 180 ms. Server xử lý tuần tự: khi daemon lạnh, một truy vấn sâu (52,8 s) chặn mọi câu khác, kể cả câu `--depth 1` (chờ 53 s).
- Không thấy `exact` nào sai. 7 lần sửa code đều được phản ánh ngay (ấm 42–515 ms), và kết quả giống hệt daemon mới.

## Bảng đích: repo thật

| # | Đích (dạng mới) | Sự thật (rg + đọc tay) | codetrace `--depth 1` | Thiếu | Thừa / nhãn | ms (server/CLI) | Cỡ |
|---|---|---|---|---|---|---|---|
| t01 | `LoyaltyServiceWalletGateway::balance/credit`: bind `WalletGateway` → impl ở `AppServiceProvider.php:211`, gọi qua promoted `private readonly WalletGateway $wallet` | `AppCampaignPublicController.php:44,109`, `CampaignPlayService.php:186`; credit `:233` | đủ cả, `likely` | 0 | — | 15/76 | 1,1 KB |
| t02 | `ZaloCheckoutGateway::resolveOrder` (bind `PaymentGatewayInterface`) | `AppPaymentController.php:160,229` | đủ, `likely` | 0 | — | 13/79 | 0,8 KB |
| t03 | `ContentQuery::fill` qua `Collection::macro('withSensitive')` | `ChatbotPerformanceReportService.php:124`, `ChatbotDailyMetricService.php:1588`, `ExperienceReportBuilder.php:221` | chỉ có `CustomerDataStorageServiceProvider::registerMacros :97` (rồi `boot`) | **3** (im lặng) | gán cho boot | 26/96 | 0,9 KB |
| t03b | `ContentQuery::exclude` qua `Builder::macro('excludingCustomerStorage')` | 7 dòng: `restoreMigrateFormToWidgetOfChatMessage.php:30`, `UpdateChatMessagesFormToWidget.php:32`, `MigrateStorageStructure.php:207,551,563,650`, `BackfillAgentHarnessMessageTelemetryCommand.php:51` | chỉ có dòng provider `:96` | **7** | gán cho boot | 32/97 | 0,7 KB |
| t04 | first-class callable: `$this->normalizeToken(...)`, `self::normalizeWindow(...)`, `$this->support->presentVersion(...)` | `CapabilitySearch.php:195`; `WorkingHourWindows.php:20`; `ListBotConfigVersionsTool.php:49` + `BotConfigToolSupport.php:51` | đủ, exact | 0 | — | 11/70–81 | 0,6–0,8 KB |
| t05 | `[$this,'extractShowroomStores']` trong `array_map`; `$this->listedTs()` trong closure của `usort`/`array_filter` | `PicoFunction.php:171`; `SeedConversations.php:250,543,1069,1083,1394,1401` | đủ (`[callable]`, exact); thêm `BotFunction.php:160` dạng dynamic (hợp lý vì method là protected) | 0 | — | 11–12/74 | 1–2 KB |
| t06a | enum `AgentStreamEvent::isTerminal`: `::tryFrom(...)?->isTerminal()` | `WorkspaceAgentController.php:504`, `WorkspaceAgentEventStream.php:61` | `:504` exact, `:61` unresolved | 0 | `:61` có thể là exact (`$event = is_string($event) ? AgentStreamEvent::from($event) : $event`) | 12/76 | 1 KB |
| t06b | enum `RunStatus::isTerminal` qua `$casts` (`$current->status->isTerminal()`) | `RunRepository.php:51,70`, `WorkspaceAgentQueue.php:212` | đủ, đều unresolved | 0 | nhãn yếu | 11/70 | 1 KB |
| t07a | `PipelineStage::isTerminal` qua quan hệ `$log->toStage?->isTerminal()` và biến từ closure `$targetStage = $resolveTarget($deal)` | `DealService.php:213,954,2347` | `:213`, `:2347` unresolved | **`:954`** (im lặng) | — | 18/69 | 0,8 KB |
| t08 | scope: `EmailMailbox::scopeActive`, `DataSource::scopeSearchBySources` | `SyncEmailMailboxesCommand.php:18`, `Workflow.php:255`; `WebResponseTransformer.php:954`, `ChatService.php:440` | **0 / 0** | **2 / 2** | — | 10–12/73–82 | 0,3 KB |
| t09 | accessor: `Bot::getBotNameAttribute` (`->bot_name`), `User::hasPassword()` (`Attribute`, `->has_password`) | `CustomerLabelService.php:134` (`fn(Bot $bot) => $bot->bot_name`), `FormSubmissionFormatter.php:25`, `NotificationSubjectService.php:45,56`, `DiscordNotifier.php:45`, `$appends` ở `Bot.php:86`; `ProfileController.php:30,38`, `HandleInertiaRequests.php:138` | không có dòng nào đúng; chỉ có 1 dòng nhiễu `log_exception` ở `helpers.php:1037` | **6+ / 3** | nhiễu (lỗi 8) | 24–26/59–86 | 0,5 KB |
| t10 | middleware: `EnsurePlanModule::handle` (alias `plan.module:` ở `routes/web.php:1382`), `AddCustomerStorageNotices::handle` (nhóm web), `SetWidgetLocale::handle` (nhóm api) | mọi route của nhóm / prefix `zma` | **0 / 0 / 0**, không có entry point | toàn bộ | — | 11–12/79–82 | 0,3 KB |
| t11 | form request `CallCampaignRequest::rules/authorize` (inject vào `CallCampaignController::store/update`) | `CallCampaignController.php:75,108` và 2 route | **0 / 0** | 2 | — | 11–20/74–84 | 0,3 KB |
| t12 | `UserOffline::broadcastWith`, `ConversationHistoryCreated::broadcastOn` (framework gọi khi `broadcast(new …)`) | `UserPresenceController.php:46`, `CleanupExpiredPresence.php:33`; `ChatConversationHistoryObserver.php:18` | **0 / 0** | 2 / 1 | — | 12–23/58–84 | 0,3 KB |
| t12c | `UserOffline::__construct` (đối chứng) | 2 dòng `broadcast(new …)` | đủ, có `route POST /api/presence/go-offline` | 0 | `routes/api.php:289:289` (định dạng lặp số dòng) | 65/105 | 0,9 KB |
| t13 | `FormSubmissionObserver::saved`, gắn bằng `#[ObservedBy]` ở `FormSubmission.php:14` | sự kiện `eloquent.saved` + 6 chỗ `FormSubmission::create` | **0** | toàn bộ | — | 19/84 | 0,4 KB |
| t14 | `SyncFacebookPostCommentsJob::handle`: `new J` trong `array_map` rồi đưa vào `Bus::batch($jobs)` | `SyncFacebookPageCommentsJob.php:189` ← `:156`; `ExternalCommentController` | `--depth 3`: đủ, 3 entry point | 0 | `routes/web_api.php:1428` (group closure) thành entry point giả; `:205` (`->dispatch()` của PendingBatch) bị tính unresolved cho `::dispatch` của cả 2 job | 49/108 | 1,5 KB |
| t15 | `NhanhApi::forBot(): ?static` với `new static`, rồi `$api = $bot ? NhanhApi::forBot($bot) : null; $api->updateOrder()` | `SyncNhanhPaymentOnOrderPaid.php:63`, `NhanhApi.php:53`, `NhanhOutbound.php:420` | đủ, exact; `new static` ở `NhanhApi.php:27` là `[new]` | 0 | — | 10–21/76–96 | 0,8–1 KB |
| t15e | constructor thừa kế: `SapoProvider::__construct` (body ở `AbstractInventoryProvider.php:13`) | `new SapoProvider` ở `SapoIntegrationService.php:233` và `new static` ở `AbstractInventoryProvider.php:23` | chỉ có `:233` | `:23` | — | 10/74 | 0,6 KB |
| t15c | `AbstractInventoryProvider::__construct` (cùng body) | `:23` và `SapoIntegrationService.php:233` | chỉ có `:23` | **`:233`** (im lặng) | — | 19/84 | 0,7 KB |
| t16 | `SlackNotifier::send` qua `$notifier = match(...) { 'slack' => new SlackNotifier(...), …, default => null }` | `ProcessFormSubmissionNotificationsJob.php:147` | unresolved | 0 | nhãn có thể là `likely` | 11/78 | 0,7 KB |
| t17 | `SubscriptionPlan::isModuleEnabled`: `?->` nhiều tầng `$this->activePlanSubscription()?->subscriptionPlan?->isModuleEnabled()` | 9 dòng (`AppController.php:92`, `Workspace.php:972,998,1013,1042,1043`, `SystemReportService.php:670,673`, `SubscriptionPlan.php:468`) | đủ 9 (2 exact, 6 unresolved) | 0 | — | 10/74 | 2 KB |
| t18 | closure trong `->filter(fn …)` và `->each(function …)` gọi `$this->x()` | `WorkflowNode.php:1913`, `WorkflowManagedLabelService.php:42` | đủ, exact | 0 | — | 10–14/73–79 | 0,6 KB |
| t19 | `Bot::isCitationsEnabled`: quan hệ `$chatbot->bot->m()` | 8 dòng | đủ 8 (2 exact, 6 unresolved) | 0 | + `log_exception` nhiễu | 14/84 | 2,2 KB |
| t20 | facade `Cache::remember` | 57 dòng | hỏi `Illuminate\Support\Facades\Cache::remember` ra 52 symbol, `likely`; hỏi `Illuminate\Cache\Repository::remember` chỉ ra 1 (`AssigneeSelector.php:355`) | 56 khi hỏi theo class thật | thừa `.opencode/skills/…/mobile-android.md:396` (`[text call]` Kotlin trong markdown) | 27–36/96–126 | 0,6 / 10,9 KB |
| t22 | hàm global `onboardingEntryRoute` gọi trong route arrow fn `routes/web.php:90` | 11 nơi | đủ 11 nơi; **entry point thiếu `GET /connect/facebook`**, thay bằng "routes/web.php (top-level code)" | entry point (lỗi 3) | — | 71/143 | 3,5 KB |
| t23 | `SyncRun::isTerminal`, `Bot::shouldSendMessage` sau `::query()->find()/findOrFail()` | `SyncRunJob.php:43,59`; `AutoReplyController.php:19` | không có | **3** (im lặng, lỗi 1) | `Bot::shouldSendMessage` còn có 3 dòng nhiễu từ vendor `Model::__call/__callStatic/newEloquentBuilder` | 10–21/70 | — |

## Bảng đích: bản sao (fixture `app/Ct3/*`, provider đăng ký trong `bootstrap/providers.php`)

| # | Dạng | Dòng gọi | Kết quả | Đánh giá |
|---|---|---|---|---|
| c01/c07 | `bind(Greeter, EnglishGreeter)`; gọi qua promoted readonly, `app(Greeter::class)`, `resolve(...)` | `Ct3Usage.php:23,28` | interface: `:23` exact, `:28` likely; impl: `likely` | đúng. Bind không được dùng để thu hẹp: `FrenchGreeter` (không bind) cũng nhận `likely` |
| c01/c08 | facade tự viết `Greet::greet()` (accessor `Greeter::class`) | `Ct3Usage.php:33` | chỉ có khi hỏi `App\Ct3\Facades\Greet::greet` (likely); `Greeter::greet` và `EnglishGreeter::greet` **không có** | thiếu (lỗi 2) |
| c03 | `Str::macro('ct3Slug', fn => Ct3Helper::slug())` | `Ct3Usage.php:38` `Str::ct3Slug(' X ')` | chỉ có `Ct3ServiceProvider::boot :23` | thiếu `:38` |
| c04 | `array_map($this->helper->double(...))`, `array_map([$this->helper, 'double'])` | `:43`, `:46` | `:43` exact; **`:46` không có** | callable `[$this->prop, 'm']` bị bỏ sót (`[$this,'m']` thì được) |
| c03/c04 | `Ct3Helper::slug(...)`, `strlen(...)` | `:44` | `:44` exact | đúng |
| c05 | `usort($xs, [$this, 'compare'])`; closure trong `collect()->each()` gọi `$this->compare()` | `:47`, `:129` | đủ | đúng |
| c06 | `Ct3Status::from()->label()`, `tryFrom()?->label()`, `Ct3Status::Open->label()`, `fromLabel()->label()`, `$model->status->label()` (cast) | `:59–62`, `:86` | đủ; `:61` (enum case) và `:86` unresolved | case enum nên là exact |
| c09/c10 | `__call`, `__callStatic` | `:113` `$magic->double(2)`, `Magic::slug('Y')` | 0 / 0, và cũng không có ở `Ct3Helper::double/slug` | **thiếu, im lặng** |
| c11 | Eloquent: `::find()`, `::query()->find()`, `::findOrFail()`, `::query()->where()->first()`, `::where()->firstOrFail()`, `->parent->`, `->parent()->first()->`, `?->parent?->`, `each(fn(Ct3Model $c))` | `:70,72,73,74,75,77,78,79,80` | 70 likely, 73 likely, 75/77/78/79 unresolved, 80 exact; **72 và 74 không có** | lỗi 1 |
| c12 | scope `scopeFresh` | `:81–83` | 0 | thiếu |
| c13/c14 | accessor `getShoutAttribute` / `whisper(): Attribute` | `:84`, `:85` | 0 / 0 | thiếu |
| c15 | `#[ObservedBy([Ct3ModelObserver::class])]` | `Ct3Model::create` `:87`, `$made->save()` `:89` | 0 | thiếu |
| c16 | listener (`Event::listen` + auto-discover) với `Event::dispatch(new E)`, `E::dispatch()`, `broadcast(new E)` | `:118–120` | `[event]` ← `__construct` ← `viaEvents` | đúng (`broadcast()` thực ra không gọi listener, nên dư chút, chấp nhận được) |
| c17 | `Ct3Happened::broadcastOn` | `:120` | 0 | thiếu |
| c18 | policy `BotPolicy::update` (`Gate::policy`): `$this->authorize('update', $bot)`, `->can('update', $bot)`, `Gate::allows('update', $bot)` | `Ct3Controller.php:17,18`, `Ct3Usage.php:125` | **0** | thiếu |
| c19 | `Gate::define('ct3-ability', [Ct3Gate::class, 'check'])` | `Gate::allows('ct3-ability')` `:125` | chỉ có `Ct3ServiceProvider::boot :25 [callable]` | thiếu chỗ dùng; gán cho boot |
| c20–22 | `static::create()` trong `BaseRecord::spawn`, `ChildRecord` override `create`; `ChildRecord::spawn()->describe()` | `BaseRecord.php:9`, `Ct3Usage.php:96` | `:9` `late static, likely` cho Child, exact cho Base; `:96` exact | đúng; thừa `SyncsAppPivots.php:39` `$model::create` (unresolved, theo README) |
| c23 | controller → route | `routes/web.php` | `route PUT /ct3/bots/{bot}` + entry point giả "routes/web.php (top-level code)" | thừa (lỗi 4) |

Thời gian trên bản sao: server 9–31 ms, CLI 79–134 ms. Output 0,4–1,1 KB.

## Chuỗi sâu (không `--depth`, repo thật, `timeout 120`)

| Đích | Lần 1 | Lần 2 | Symbol | Cỡ | Entry point | Nhận xét |
|---|---|---|---|---|---|---|
| `LoyaltyServiceWalletGateway::debit` | 22/123 ms | — | 4 | 1 KB | 2: `route POST /api/apps/{appType}/{appId}/campaigns/{campaign}/grant-turns` + `routes/api.php:349 (top-level)` giả | đúng, trừ entry giả |
| `SubscriptionPlan::isModuleEnabled` | 518/677 ms | — | 151 (88 phía trên, 37 file) | 12 KB | 48 = 43 route + 5 artisan, trong đó **11 top-level giả** | hợp lý (webhook kênh, developer API, retrain) |
| `Bot::isCitationsEnabled` | 1.659/1.802 ms | — | 369 (283 phía trên, 84 file) | 28 KB | 63 = 56 route + 6 artisan, **17 top-level giả** | hợp lý |
| `ClassifyDirtyConversationsJob::handle` (đích treo của vòng 2) | **23.792/23.864 ms**, `Cost: … a driver restarted` | 180/251 ms | 836 (606 phía trên, 203 file) | 59 KB | 132 = 114 route + 17 artisan, **30 top-level giả**; scheduler chỉ hiện là `routes/console.php (top-level code)` | kết quả 2 lần giống hệt; lần 1 dính watchdog 20 s (lỗi 6) |

Ba truy vấn kiểm thêm: `CleanupExpiredPresence::handle` có entry point duy nhất là `routes/console.php (top-level code: runs when the file is loaded)`, không nói đây là `Schedule::job`. `SyncFacebookPostCommentsJob::handle --depth 2` đã ra nơi dispatch thật (vòng 2 cần `--depth 3`): mục depth của job đã được sửa.

## `locate`

| Dòng | Nội dung | Trả về | Đánh giá | ms |
|---|---|---|---|---|
| `app/Ct3/Ct3Usage.php:80` | `->each(fn (Ct3Model $child) => $child->ping())` | `Ct3Usage::viaEloquent :67` | đúng | 110 |
| `app/Ct3/Ct3Usage.php:103` | match arm `default => new FrenchGreeter()` | `Ct3Usage::viaMatch :99` | đúng | 90 |
| `app/Ct3/Ct3Usage.php:129` | `$this->compare()` trong `collect()->each(function …)` | `Ct3Usage::viaCollection :126` | đúng | 110 |
| `app/Ct3/Ct3Usage.php:16` | promoted `private readonly Greeter $greeter,` | `Ct3Usage::__construct :15` | đúng | 89 |
| `app/Ct3/Ct3Usage.php:132` | `}` cuối class | `App\Ct3\Ct3Usage :13` | đúng | 98 |
| `app/Ct3/Ct3Status.php:7` | enum case `case Open = 'open';` | `App\Ct3\Ct3Status :5` | đúng | 80 |
| `app/Ct3/Ct3Model.php:13` | attribute class `#[ObservedBy(...)]` | `app/Ct3/Ct3Model.php` (file) | chấp nhận được; có thể là class | 123 |
| `app/Ct3/Facades/Greet.php:9` | docblock class `@method static …` | file | chấp nhận được | 91 |
| `app/Http/Controllers/WorkflowController.php:40` | trong `#[AgentAction(...)]` của method `index` | `WorkflowController` (class) | nên là `::index` (attribute thuộc method) | ~90 |
| `app/Services/Commerce/Outbound/Outbound.php:64` | docblock `@param` của method | `Outbound` (class) | nên là method bên dưới | ~90 |
| `app/Ct3/Ct3ServiceProvider.php:23` | `Str::macro(..., fn => ...)` | `Ct3ServiceProvider::boot :21` | đúng | 100 |
| `app/Providers/CustomerDataStorageServiceProvider.php:85` | thân closure `Builder::macro('whereSensitive', function …)` | `::registerMacros :82` | đúng về cú pháp | 134 |
| `routes/web.php:218` | thân closure route `/` | `routes/web.php:215` | đúng | 91 |
| `routes/web_api.php:1429` | trong `->group(function () {` | `routes/web_api.php:1428` | đúng về cú pháp | 115 |

## Sửa code trong lúc hỏi (bản sao)

"Ấm" là câu hỏi ngay sau khi lưu file. "Mới" là sau khi `pkill -f "[r]elease/codetrace serve <bản sao>$"`. Daemon mới trả lời trong lúc vẫn đang index, kết quả giống câu ấm, đúng như README hứa.

| # | Sửa | Hỏi | Kết quả | Ấm | Mới | Ấm = mới |
|---|---|---|---|---|---|---|
| E1 | thêm `CampaignPlayService::ct3Peek()` gọi `$this->wallet->balance()` (qua interface) | `LoyaltyServiceWalletGateway::balance` | +`:157` likely; `grantTurns` dời từ `:186` sang `:191` đúng | 341 ms | 2.461 ms | có (chỉ khác dòng `Waited`) |
| E2 | `Ct3Factory::make(): EnglishGreeter` đổi thành `FrenchGreeter`; caller `Ct3Consumer.php` không đổi | `FrenchGreeter::greet` / `EnglishGreeter::greet` | `Ct3Consumer::run` exact chuyển sang French, biến mất khỏi English | 130 / 54 ms | 1.829 / 1.276 ms | có |
| E3 | thêm `use \App\Traits\SanitizesAiResponse;` vào `Ct3Consumer` (đã có sẵn `$this->sanitizeAiResponse()`) | trait method | 7 lên 8 exact | 135 ms | 2.500 ms | có |
| E4a | xoá file caller `AppCampaignPublicController.php` | `…::balance` | mất 2 dòng | 162 ms | 1.541 ms | có |
| E4b | xoá file định nghĩa `FrenchGreeter.php` | `FrenchGreeter::greet` | in `FrenchGreeter::greet` / `Direct callers (0)`, không báo symbol không còn tồn tại | 145 ms | 1.146 ms | có (lỗi 5) |
| E5 | đổi tên `NhanhApi::updateOrder` thành `updateOrderV2` (chỉ định nghĩa) | tên mới / tên cũ | tên mới: 0 (đúng); tên cũ: 1 dòng `docs/journals/…md:45`, 3 dòng PHP còn gọi tên cũ không hiện | 94 / 42 ms | 1.128 / 1.102 ms | có (lỗi 5) |
| E6 | thêm `Route::get('/ct3/closure', function (Ct3Helper $helper) {…})` | `Ct3Helper::double` | +`routes/web.php:1815`, entry `route GET /ct3/closure` | 179 ms | 1.674 ms | có |
| E6c | thêm `Route::post('/ct3/closure2', fn (Ct3Helper $h) => $h->double(4))` | như trên, không `--depth` | caller là `routes/web.php [call]`; entry là `routes/web.php (top-level code)`; **không có `route POST /ct3/closure2`** | 12 ms | 1.157 ms | có, sai cả hai bên (lỗi 3) |
| E7 | `bind(Greeter, EnglishGreeter)` đổi thành `FrenchGreeter` | `FrenchGreeter::greet`, `EnglishGreeter::greet` | không đổi gì (bind không được dùng); facade `Greet::greet` vẫn không nối | 148 / 79 ms | 1.089 ms | có |

## Hai client cùng lúc

- Daemon ấm, 4 lệnh song song (2 truy vấn sâu, 2 truy vấn `--depth 1/2`): xong trong 250–634 ms. `Waited: 233–550 ms` cho thấy server xử lý tuần tự. Cả 4 output giống hệt khi chạy tuần tự. Không treo, không sai.
- Daemon lạnh (bản sao), 4 lệnh song song + sửa `Ct3Helper.php` giữa chừng + 1 lệnh vào repo thật: lệnh vào repo thật xong sau 170 ms. Ở bản sao, truy vấn sâu `Bot::isCitationsEnabled` mất 52,8 s; ba câu nhỏ (`--depth 1`) phải **chờ 52,9–53,3 s** dù chỉ cần 0,3–2 s để tính. Kết quả cuối vẫn đúng: bản sao và repo thật giống hệt nhau, và câu hỏi sau lần sửa phản ánh đúng.

## Lỗi theo mức nặng

### Nặng

**1. `::query()->…->find()/first()/findOrFail()` rồi gọi method: bỏ sót im lặng**
- Repo thật: `SyncRunJob.php:42-43` (`$run = SyncRun::query()->with('bot')->find($this->runId); … $run->isTerminal()`) và `:58-59` không có ở `SyncRun::isTerminal`, kể cả unresolved. `AutoReplyController.php:14,19` (`$bot = Bot::query()->findOrFail($botId); $bot->shouldSendMessage()`) không có ở `Bot::shouldSendMessage`.
- Bản sao, `app/Ct3/Ct3Usage.php`: `:72` (`$b = Ct3Model::query()->find($id); $b->ping()`) và `:74` (`Ct3Model::query()->where(...)->first()->ping()`) không có. Các dạng tĩnh thì được: `:70` `Ct3Model::find` (likely), `:73` `::findOrFail` (likely), `:75` `::where()->firstOrFail()` (unresolved).
- Giả thuyết về nguyên nhân, có đối chứng: khi `vendor/laravel` nằm ngoài repoRoot (symlink), `SyncRunJob.php:43,59` hiện ra dạng `unresolved`. Khi copy `vendor/laravel` vào trong repo thì hai dòng **biến mất**. Nghĩa là khi đọc được `Builder::find()` (docblock điều kiện `@return ($id is (Arrayable|array) ? Collection<int, TModel> : TModel|null)`, `Builder.php:469`), tool suy ra một kiểu "đã biết nhưng không phải SyncRun" rồi bỏ cạnh này đi. Trường hợp không biết kiểu còn tốt hơn trường hợp suy sai.

**2. Các method Laravel gọi theo quy ước ra 0 caller, không báo gì (agent sẽ tưởng code chết)**
- scope: `EmailMailbox::scopeActive` (thật: `SyncEmailMailboxesCommand.php:18`, `Workflow.php:255`), `DataSource::scopeSearchBySources` (`WebResponseTransformer.php:954`, `ChatService.php:440`): 0.
- accessor: `Bot::getBotNameAttribute` (vd. `CustomerLabelService.php:134` `fn(Bot $bot) => $bot->bot_name`, kiểu đã khai báo rõ), `User::hasPassword()` (`HandleInertiaRequests.php:138`), `Ct3Model::getShoutAttribute/whisper`: 0.
- middleware: `EnsurePlanModule::handle` (`bootstrap/app.php:105` alias, `routes/web.php:1382`), `AddCustomerStorageNotices::handle` (`bootstrap/app.php:65`, nhóm web), `SetWidgetLocale::handle` (nhóm api): 0, không có entry point nào. Tool đã khởi động Laravel để đọc route, nên `Route::getRoutes()` + `gatherMiddleware()` là có sẵn.
- form request: `CallCampaignRequest::rules/authorize` (inject ở `CallCampaignController.php:75,108`): 0.
- policy: `BotPolicy::update` qua `Gate::policy` + `$this->authorize('update', $bot)` (`Ct3Controller.php:17`), `->can()`, `Gate::allows()`: 0. `Gate::define([C,'m'])` thì có cạnh `[callable]`, nhưng cạnh đó trỏ về `boot`, không về chỗ `Gate::allows('ct3-ability')`.
- `#[ObservedBy]`: `FormSubmissionObserver::saved` (`FormSubmission.php:14`), `Ct3ModelObserver::saved`: 0. Vòng 2, `Model::observe()` ra được `event eloquent.created`; dạng attribute thì chưa.
- `broadcastOn/broadcastWith` (`UserOffline.php:38`, `ConversationHistoryCreated.php:36`): 0.
- `__call/__callStatic`: `Magic::__call`, `Magic::__callStatic` đều 0, và `Ct3Usage.php:113` (`$magic->double(2)`, `Magic::slug('Y')`) không có ở đâu cả.
- macro: chỉ có dòng đăng ký trong provider; mất toàn bộ chỗ gọi (t03: 3 dòng, t03b: 7 dòng, c03: 1 dòng). Riêng `withSensitive`/`excludingCustomerStorage` là đường đọc/ghi dữ liệu nhạy cảm của khách.
- facade: hỏi theo tên facade (`Cache::remember`, `Greet::greet`) thì ra, nhưng hỏi theo class thật (`Illuminate\Cache\Repository::remember`: 1/57; `EnglishGreeter::greet`, `Greeter::greet`: thiếu `Ct3Usage.php:33`) thì không.

Đề xuất: tool đã có các cạnh `[queue]`, `[event]`, `[artisan]`, `[command]`. Cần thêm cạnh theo quy ước cho scope (`->x()` → `scopeX`), accessor (`->x_y` → `getXYAttribute` / `xY(): Attribute`), middleware → route, form request → action, policy → `authorize/can/Gate`, `#[ObservedBy]` → `eloquent.*`, macro → chỗ gọi, facade → `getFacadeAccessor`/bind. Trước mắt, ít nhất phải in "method được framework gọi theo quy ước, chưa lần được" thay vì `Direct callers (0)`.

**3. Route viết bằng arrow fn mất entry point, bị coi là code chạy lúc nạp file**
- `routes/web.php:90` `Route::get('/connect/facebook', fn() => redirect()->route(onboardingEntryRoute()))`: caller là `routes/web.php [function]`. Entry point có `routes/web.php (top-level code: runs when the file is loaded)` và không có `GET /connect/facebook`.
- Tái hiện E6c: `Route::post('/ct3/closure2', fn (Ct3Helper $h) => $h->double(4))` thì sai. `function (...) {}` ở E6 thì đúng (`route GET /ct3/closure`).
- Phạm vi: `rg` đếm được 177 route dạng `fn` trong `routes/web.php`, 2 trong `routes/web/guest.php`, 1 trong `routes/web_api.php`.

### Trung bình

**4. Group closure bị tính là entry point giả**
- Mọi `Route::prefix(...)->group(function () {` (`routes/web_api.php:209,894,1181,1428`, `routes/api.php:225,349`, `app/Agent/Adapters/Web/Http/routes.php:11`) đều thành caller của controller method và thành entry point "top-level code: runs when the file is loaded". Ví dụ t14: `routes/web_api.php:1428:1428` (số dòng còn bị lặp).
- Tỉ lệ: 11/48 (`isModuleEnabled`), 17/63 (`isCitationsEnabled`), 30/132 (`ClassifyDirtyConversationsJob`). Ngay cả route thường (c23) cũng kèm thêm `routes/web.php (top-level code)`. Câu "runs when the file is loaded" là sai về ngữ nghĩa: controller không chạy khi nạp file route.

**5. Symbol không tồn tại: không báo lỗi, trả về kết quả grep chữ**
- `callers 'App\Nope\Fake::bar'`: rc=0, 11 "caller" `[text call, unresolved]` từ `.opencode/skills/ui-ux-pro-max/data/charts.csv:14`, `docs/zma/affiliate.md:127`, `plugins/zma-template/src/static/vn-geo/wards.json:1`, `resources/js/lib/ckeditor5/build/ckeditor.js.map:1`… (khớp chữ "bar" không phân biệt hoa thường, trong văn xuôi).
- Gõ sai `Ct3Helper::doubel`: 2 caller dynamic. File đã xoá (E4b): `Direct callers (0)`. Tên cũ sau khi đổi tên (E5b): 1 dòng markdown, trong khi 3 dòng PHP còn gọi tên cũ thì không hiện.
- Agent không phân biệt được "không ai gọi" với "gõ sai tên". Cần một dòng `symbol not found` (và rc khác 0) khi không có định nghĩa. Driver chung cũng không nên quét `.csv/.json/.map`.

**6. Driver Laravel vẫn bị kẹt; watchdog cắt sau 20 s, nhưng server tuần tự nên cả hàng đợi phải chờ**
- Log `/tmp/codetrace-robertnguyen/d73c3724e49463dd.log`: `laravel: no answer and no work for 20 s; restarting it. Waiting for: {"id":1054,"op":"callers","symbol":"App\\Services\\Commerce\\Inbound\\Inbound::lowerStatusBack",…}` lúc chạy `ClassifyDirtyConversationsJob::handle` không giới hạn depth (cùng đích làm treo ở vòng 2). Lần 1 mất 23,9 s, lần 2 mất 180 ms, output giống hệt. Hỏi riêng `Inbound::lowerStatusBack --depth 1` sau đó chỉ mất 16 ms, nên chỗ kẹt không cố định theo symbol.
- Tiến bộ so với vòng 2: không còn treo vô hạn, kết quả vẫn đủ. Nhưng nguyên nhân treo chưa được sửa, và trong 20 s đó mọi client khác phải chờ.
- Cùng gốc: khi daemon lạnh, một truy vấn sâu (52,8 s) chặn 3 câu `--depth 1` trong 53 s. README nói câu hỏi trong lúc index "mất vài giây".

**7. Callable `[$this->prop, 'm']` bị bỏ sót**: `Ct3Usage.php:46` `array_map([$this->helper, 'double'], $xs)` không có ở `Ct3Helper::double`. `[$this,'m']` (`:47`) và `$this->helper->double(...)` (`:43`) thì được.

**8. Constructor thừa kế: hai đồ thị không nối nhau** (cùng dạng với lỗi trait vòng 2)
- `AbstractInventoryProvider::__construct` thiếu `SapoIntegrationService.php:233` (`new SapoProvider`, SapoProvider không có constructor riêng). `SapoProvider::__construct` (locate trỏ về cùng body `AbstractInventoryProvider.php:13`) thiếu `new static` ở `AbstractInventoryProvider.php:23`.

**9. Biến lấy từ closure-variable `$x = $fn(...)` mất dấu**: `DealService.php:939,954` (`$targetStage = $resolveTarget($deal); $targetStage->isTerminal()`) không có ở `PipelineStage::isTerminal`, kể cả unresolved. README hứa trường hợp không rõ kiểu sẽ được liệt kê `unresolved`.

### Nhẹ

- **Nhiễu `log_exception`**: `helpers.php:1037` `logger()->$level(...)` hiện thành `[dynamic, unresolved]` cho mọi method của `Bot`, `User`, `Workspace` (`getBotNameAttribute`, `hasPassword`, `scopeActive`, `getConfigAttribute`, `isCitationsEnabled`, `canAccessAffiliate`…) nhưng không cho `SubscriptionPlan`, `ChatConversation`, `Ticket`, `EmailMailbox`, `WorkflowNode`. Với `Bot::shouldSendMessage` còn thêm 3 dòng vendor `Model::newEloquentBuilder/__call/__callStatic`.
- Bind trong provider không được dùng: mọi class cài đặt interface đều `likely`, và E7 (đổi bind) không làm gì thay đổi. Chấp nhận được, nhưng README nên nói rõ.
- Nhãn yếu: enum case `Ct3Status::Open->label()` (`Ct3Usage.php:61`) và enum qua `$casts` (`RunRepository.php:51`) là unresolved, có thể exact. `match` mà mọi nhánh đều là object (`Ct3Usage.php:106`, `ProcessFormSubmissionNotificationsJob.php:147`) là unresolved, có thể likely. Anonymous `fromRaw` ← `Inbound.php:3148` là unresolved trong khi README hứa `likely`.
- Cạnh "đăng ký" bị gán cho `boot()`: macro, `Gate::define`, `Queue::after(fn() => app(X)())` (`AppServiceProvider.php:376`) đều thành caller trong `AppServiceProvider::boot`. Đi lên tiếp thì thành "chạy lúc khởi động app", không phải nơi thực sự kích hoạt.
- `Bus::batch(...)->dispatch()` (`SyncFacebookPageCommentsJob.php:205`) bị tính unresolved cho `::dispatch` của cả hai job.
- Scheduler: `CleanupExpiredPresence::handle` chỉ có entry point `routes/console.php (top-level code)`, không nói là `Schedule::job` (`routes/console.php:87`).
- `locate` ở dòng attribute hay docblock của method trả về class thay vì method (`WorkflowController.php:40` thuộc `#[AgentAction]` của `index`; `Outbound.php:64`).
- Tài nguyên: daemon backend có driver PHP 699 MB RSS + 6 tiến trình con khoảng 634 MB RSS mỗi cái (4 con idle hơn 13 phút). Dòng `Cost` báo khoảng 1,0–1,3 GB "proportional". Daemon bản sao để lại 8 tiến trình `[php] <defunct>` chưa reap. Mọi output trên bản sao in `Warning: typescript is not installed…` (đúng, vì bản sao không có `node_modules`).

## Chạy đúng

- Interface bind: gọi qua promoted readonly property, `app()`, `resolve()` đều ra (exact trên interface, likely trên class cài đặt). Đổi kiểu trả về làm chỗ gọi chuyển class ngay (E2).
- First-class callable đủ 4 dạng (`$this->m(...)`, `self::m(...)`, `$this->prop->m(...)`, `C::m(...)`); `[$this,'m']` trong `array_map`/`usort`; closure trong `each/filter/usort/array_filter`.
- `Enum::from()->m()`, `tryFrom()?->m()`, `Enum::staticFactory()->m()`.
- `static::create()` / `new static` với kiểu trả về `?static` hoặc `static` (`NhanhApi::forBot`, `BaseRecord::spawn`), kể cả kiểu ternary-null.
- `?->` nhiều tầng, quan hệ Eloquent: luôn được liệt kê (unresolved), không sót.
- Bus batch, `Event::dispatch`, `E::dispatch()`, `broadcast(new E)` đến listener; `Gate::define([C,'m'])` thành `[callable]`.
- `locate`: 14 dòng khó đều trả về đúng hoặc chấp nhận được, mỗi lần 80–134 ms (CLI).
- Daemon theo dõi thay đổi tốt: 7 kiểu sửa (thêm chỗ gọi, đổi kiểu trả về, thêm trait, xoá file caller/định nghĩa, đổi tên, thêm route, đổi bind) đều phản ánh ngay (42–515 ms) và giống hệt daemon mới. Truy vấn trong lúc daemon mới đang index cũng cho đúng kết quả.
- Chạy song song: không treo, kết quả giống chạy tuần tự.
- Các mục của vòng 2 đã sửa: trait, ternary-null, `$class::m()`, `$this->$m()`, anonymous class, `Artisan::call`, `app(X::class)()`, `GET /` closure, job ở `--depth 2`, không còn cảnh báo `BusinessObjectPaymentTest.php`.
