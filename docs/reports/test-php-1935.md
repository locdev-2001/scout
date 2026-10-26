# Test codetrace trên PHP/Laravel (backend) — 25/10/2026 19:35

Repo: `repos/easy-ai-platform-backend` (`develop`, chỉ đọc). Gọi qua MCP `codetrace` (`callers`, `locate`) với `repo` là đường dẫn tuyệt đối của backend; vài lần kiểm thêm bằng CLI cùng daemon. Đối chiếu độc lập bằng `rg` + đọc code.

## Tóm tắt

- Người gọi trực tiếp: **4/4 đích khớp hoàn toàn** với `rg` + đọc tay; không sót, không có `exact` sai. Thêm 2 đích phụ (method contract `WalletGateway::credit`, bản cài đặt qua registry `InstantWinHandler::resolvePlay`) cũng khớp.
- Tầng lan truyền phía trên: **sai nặng do cạnh `likely` ảo** trên `Model::save()` gọi theo chuỗi (`$x->forceFill([...])->save()`), khiến một hàm có 10 chỗ gọi thật thành 1.444 symbol / 238 entry point, gồm cả route không liên quan.
- Lần hỏi đầu qua MCP **treo 1.800 s rồi bị huỷ**, không có câu trả lời, không có log. Sau khi daemon mới lên: lần đầu 65,7 s; các lần sau 8–50 ms.

## Kết quả theo đích

| # | Đích (lý do chọn) | Người gọi trực tiếp theo rg/đọc tay | Theo codetrace | Bỏ sót | `exact` sai | `likely` sai | Thời gian (`Cost:`) |
|---|---|---|---|---|---|---|---|
| 1 | `App\Services\AutoAssignService::assignMember` — service gọi ở nhiều nơi (Chatbot, Messenger, job, analyzer) | 10 dòng / 8 method | 10 dòng / 8 method (Chatbot gộp 2 dòng/method) | 0 | 0 | trực tiếp 0; tầng trên: nhiều (xem lỗi 1) | MCP lần đầu: treo, huỷ sau 1.800 s; CLI lần đầu 65.756 ms wall, ~178 s CPU, 990 MB; lặp lại 36–50 ms |
| 2 | `App\Services\Wallet\LoyaltyServiceWalletGateway::credit` — chỉ được gọi qua interface `WalletGateway` (bind ở `AppServiceProvider.php:211`) | 1 (`CampaignPlayService.php:233`, `$this->wallet->credit`, `$wallet` kiểu `WalletGateway`) | 1, `likely` | 0 | 0 | 0 | 40 ms |
| 3 | `App\Listeners\AwardLoyaltyOnOrderPaid::handle` — listener xếp hàng (`ShouldQueue`), tự đăng ký (auto-discover), không ai gọi trực tiếp | sự kiện `OrderPaid`; phát ở `OrderEffects.php:99`, `ReconcileLoyaltyAwardCommand.php:75` | `OrderPaid::__construct [event]`; hai nơi phát có ở tầng trên (`OrderEffects::changed`, `ReconcileLoyaltyAwardCommand::handle`) | 0 | 0 | không kiểm hết 86 `likely` của tầng trên | 21 ms |
| 4 | `App\Http\Controllers\Apps\AppCampaignPublicController::play` — controller có route | `routes/api.php:428` (POST `/campaigns/{campaign}/play`) | `routes/api.php:428 [callable]` + `route POST /api/apps/{appType}/{appId}/campaigns/{campaign}/play` | 0 | 0 | 0 | 9 ms |
| phụ | `App\Contracts\Wallet\WalletGateway::credit` | 1 | 1, `exact` | 0 | 0 | 0 | 12 ms |
| phụ | `App\Services\Campaign\Mechanics\InstantWinHandler::resolvePlay` (gọi qua `CampaignMechanicHandler` lấy từ registry) | 1 (`CampaignPlayService.php:87`) | 1, `likely`, lên tới route `/play` | 0 | 0 | 0 | 21 ms |
| phụ | `AutoAssignService::takeOverAsManual` | 3 (`ConversationController.php:2220`, `WebApi/OrderController.php:82`, `ChatConversation.php:2839`) | 3 | 0 | 0 | 0 | 28 ms |
| đối chứng | `AutoAssignService::getNextAdminByMode` | 0 (hai chỗ gọi tên này ở `RuleActionResolver.php:37`, `AutoAssignService.php:262` đều là `$this->selector->`, kiểu `AssigneeSelector`) | 0 | 0 | 0 | 0 | 10 ms |

Chuỗi đích 2 và 4 được kiểm tới tận gốc: `fulfill` ← `play` (`CampaignPlayService.php:141`) ← `AppCampaignPublicController::play` (`:70`) ← route; `fulfill` ← `refulfill` (`:267`) ← `ReconcileCampaignRewards::handle` (`ReconcileCampaignRewards.php:39`) ← `artisan campaign:reconcile-rewards`. Khớp hoàn toàn với output.

`unresolved` thừa vô lý: không thấy. Các dòng `unresolved` của đích 3 đều là gọi động thật (`app/Agent/Tools/ToolRegistry.php:112`, `app/Models/WorkflowNode.php:318`, `SeedEcommerceDemoCommand.php:141`).

## `locate`

| Dòng | Nằm ở | codetrace trả | Đánh giá |
|---|---|---|---|
| `app/Services/AutoAssignService.php:407` | trong method `assignMember` | `AutoAssignService::assignMember` `:400` | đúng |
| `app/Services/Booking/BookingTransitionRecorder.php:484` | trong closure của `finishProviderFailure` | `BookingTransitionRecorder::finishProviderFailure` `:461` | đúng (trả method bao ngoài, không nói là closure) |
| `app/Models/ChatMessage.php:573` | closure lồng 2 tầng trong `scopeFilter` | `ChatMessage::scopeFilter` `:550` | đúng |
| `app/Listeners/AwardLoyaltyOnOrderPaid.php:26` | property `$queue`, ngoài method | class `AwardLoyaltyOnOrderPaid` `:24` | đúng |
| `app/Listeners/AwardLoyaltyOnOrderPaid.php:5` | dòng `use`, ngoài class | file `app/Listeners/AwardLoyaltyOnOrderPaid.php:1` | đúng |
| `routes/api.php:428` | code cấp file | `routes/api.php:1` | đúng |

Mỗi lần 8–10 ms. Quan sát: frame `{closure}` trong stack trace sẽ được gán cho method bao ngoài; đủ để gọi `callers`, nhưng agent không biết dòng đó chạy trong closure (có thể chạy trễ, ví dụ trong `DB::transaction`).

## Lỗi tìm được

### 1. Cạnh `likely` ảo qua `->forceFill(...)->save()` làm sai vùng ảnh hưởng (nặng)

`callers 'App\Models\ChatMessage::save'` (method này override `Model::save`, `ChatMessage.php:289`) trả 140 người gọi trực tiếp, 111 là `likely`; **104 trong số đó là chuỗi `)->save(`** trên model khác hẳn:

- `BookingSettingsNormalizer.php:129` và `:157`: `$bot->forceFill([...])->save();`, `$bot` khai báo `Bot $bot` (`:114`, `:137`).
- `BookingTransitionRecorder.php:489`: `$transition->forceFill([...])->save();`, `$transition` khai báo `BookingEventTransition $transition` (`:463`).
- `AffiliateParticipantService.php:227`, `:263`: `$participant->forceFill(...)->save()`, `$source->forceFill(...)->save()`.

Kiểm phân biệt: `$bot->save()` không qua chuỗi (`CreateAiAgentController.php:470`, `BackfillOnboardingDraftKeyCommand.php:43`, `AddGenderFieldToContactWidgets.php:97`) **không** bị liệt kê. Vậy lỗi nằm ở chỗ tool mất kiểu khi đi qua `forceFill()` (kiểu trả về `$this` chỉ có trong docblock của Laravel, `Model.php:576`) rồi rơi về "mọi lớp con của `Model` có override `save`", gắn `likely`. Theo README, `likely` là "gọi qua class cha hoặc interface"; ở đây receiver là class anh em, nên phải bị loại (theo kiểu `$this`) hoặc tệ nhất là `unresolved`.

Thêm một cạnh sai: `ChatMessage::save [static, likely] ChatMessage.php:293 parent::save($options)` — `parent::save` gọi `Model::save`, không gọi lại chính nó.

Hậu quả với đích 1: `ChatMessage::save` → `commitPendingForDeliveredMessage` (`ChatMessage.php:297`) → `ChatHandoverAnalyzerService::requestHandover` → `assignMember`. Mọi `forceFill()->save()` trong repo vì thế thành người gọi gián tiếp của `assignMember`: 1.444 symbol, 373 file, 238 entry point, gồm `route PUT|PATCH /web_api/workspaces/{slug}/affiliate/participants/{participant}` và `app/Agent/Tools/Booking/CancelBooking.php:94` (chuỗi `CancelBooking::execute` → `BookingLifecycle::cancel` → `cancelOne` → `finishProviderFailure` → `])->save()`). Agent đọc "238 entry point" sẽ hoặc tin sai, hoặc bỏ qua cả phần tầng trên. Đích 3 (`OrderPaid`, 323 symbol, 86 `likely`) nhiều khả năng bị thổi phồng theo cùng cách; chưa kiểm từng cạnh.

### 2. Truy vấn đầu tiên treo 30 phút, im lặng (nặng)

Diễn biến:

- 18:57 gọi MCP `callers` cho đích 1. 1.800 s sau Claude Code huỷ: `MCP server "codetrace" tool "callers" sent no response or progress for 1800s`.
- 19:27 daemon `codetrace serve .../easy-ai-platform-backend` (pid 1109785) sống 30:18, CPU 0,1 %, còn đúng một con là `driver-generic` (không thấy driver PHP/Laravel/JS dưới nó); luồng chính chờ `futex`, 8 luồng đọc pipe chờ `anon_pipe_read`. File log `/tmp/codetrace-robertnguyen/c6124c19f8853393.log` 0 byte. Vài giây sau tiến trình thành `<defunct>` (cha là `codetrace mcp` không `wait`, để lại zombie).
- Cùng lúc phiên khác (`codetrace callers resources/js/utils/currencyValue.js#normalizeCurrencyValue --compact`, pid 1118867) cũng treo 29:37 trên cùng daemon.

Đã chứng minh từ source:

- `src/driver.rs:198-222` `receive()` chờ `self.responses.recv()` không có hạn; driver còn sống mà không trả lời thì daemon chờ mãi.
- Daemon xử lý truy vấn tuần tự, nên một truy vấn kẹt chặn mọi client khác.
- `src/daemon.rs:18` `IDLE_SECONDS = 30 * 60`; `last_activity` chỉ được ghi sau khi trả lời xong, nên watchdog (`daemon.rs:158-163`) `exit(0)` daemon đúng 30 phút sau khi khởi động dù đang có truy vấn. Đó là thứ "giải thoát" lần treo, không phải tự phục hồi.
- Client `query()` (`daemon.rs:66-72`) `read_line` không có hạn; `STARTUP_WAIT` 15 s chỉ bao phần kết nối.
- `start_server` (`daemon.rs:41`) `File::create` log mỗi lần khởi động, nên log của daemon trước bị xoá trắng.

Chưa chứng minh: vì sao truy vấn kẹt. Nghi vấn (chưa tái hiện): ba phiên test khởi động cùng lúc (3 tiến trình `codetrace mcp` cùng 30:43 tuổi) cùng thấy chưa có socket, cùng `start_server`; `ipc::listen` (`src/ipc.rs:21`) `remove_file` socket cũ trước khi `bind`, nên daemon khởi động sau làm mồ côi daemon trước. Máy lúc đó còn 6,9 GB RAM trống, có 4 daemon codetrace (~1 GB/daemon backend).

Lần đầu sau khi daemon mới lên: 65.756 ms wall, ~178 s CPU, 990 MB, vượt mốc "10–40 s" của đề bài (có tranh CPU với các daemon khác).

### 3. Output gọn có lỗi trình bày (nhẹ)

- Nhóm đường dẫn rỗng: dòng 14 của output đích 1 là `- : App\Agent\Events\AgentTurnCompleted::__construct, ...` dài 17.883 ký tự. Trong nhóm này có `App\Jobs\BeeskyPollJob::__construct`, có thật ở `app/Jobs/BeeskyPollJob.php:45`, nhưng không được xếp dưới file của nó; các method `dispatch*`/`broadcast` lấy từ trait `Dispatchable` cũng không có file. Agent không định vị được.
- Output gọn của đích 1 là 130 KB / 680 dòng; với `ChatMessage::save` là 145 KB. Phần lớn đến từ lỗi 1.
- Phần `unresolved` gợi ý chạy `codetrace callers '...' --depth 1`. Cờ này có ở CLI (chạy được), nhưng công cụ MCP `callers` không có tham số `depth`, nên agent chỉ dùng MCP không làm theo được.
- Với listener (đích 3), người gọi trực tiếp chỉ là `OrderPaid::__construct [event]`, không kèm `file:dòng` nơi phát sự kiện; hai nơi phát (`OrderEffects.php:99`, `ReconcileLoyaltyAwardCommand.php:75`) chỉ hiện tên symbol ở tầng trên, không có dòng.

### 4. Cảnh báo file không phân tích được: đúng, nhưng lặp

Mọi câu trả lời (cả `locate`) kèm `Warning: tests/Feature/CRM/BusinessObjectPaymentTest.php could not be analysed (PHP aborted ...)`. Đã kiểm: cảnh báo đúng, file dùng trait `Tests\Concerns\SyncsBusinessObjects` không tồn tại (`tests/Concerns/` chỉ có `AssertsGoldenPayload.php`, `CreatesBusinessObjects.php`; `ReflectionClass` báo `Fatal error: Trait ... not found`). Đây là lỗi của repo, tool báo thẳng là đúng. Góp ý: `locate` một file khác không cần cảnh báo này.

## Đánh giá output cho agent

- Phần "Direct callers" rất tốt: đủ `file:dòng#hash`, dòng code, độ tin cậy; trong mọi đích tôi kiểm, đây là đúng và đủ để biết chỗ sửa ảnh hưởng ai trực tiếp. Route, lệnh artisan, sự kiện, queue, binding interface, registry đều được nối.
- Phần "Further up" và "Affected entry points" chưa dùng được để đo vùng ảnh hưởng chừng nào lỗi 1 còn: nó kéo cả các route không liên quan, và độ dài (130–145 KB) vượt mức hợp lý cho context của agent. Với đích hẹp (2, 4) output 1–2 KB, rất gọn.
- Tốc độ khi daemon đã nóng đạt mục tiêu (8–50 ms). Lần đầu và trường hợp treo thì không: agent có thể chờ 30 phút không nhận gì.

## Đề xuất (để chủ dự án quyết)

1. Theo kiểu trả về `$this`/`static` (docblock) qua chuỗi fluent của Eloquent; khi vẫn mất kiểu thì gắn `unresolved`, không gắn `likely` cho mọi override. Bỏ cạnh `parent::m()` → chính `m`.
2. Đặt hạn chờ cho `receive()` và cho client; khi quá hạn trả lỗi kèm stderr của driver. Ghi `last_activity` cả khi bắt đầu truy vấn. Không xoá socket của daemon khác đang sống khi khởi động (khoá file hoặc thử kết nối lại sau `bind` thất bại). Mở log ở chế độ nối thêm.
3. Xếp symbol từ trait/không có file vào file của class dùng nó; thêm `depth` cho công cụ MCP hoặc đổi gợi ý.

Status: DONE_WITH_CONCERNS
Người gọi trực tiếp đúng và đủ ở cả 4 đích (cộng 4 đích phụ), `locate` đúng 6/6, truy vấn nóng 8–50 ms; nhưng cạnh `likely` ảo qua `forceFill()->save()` làm vùng ảnh hưởng phình sai (10 chỗ gọi → 238 entry point), và lần hỏi đầu qua MCP treo im lặng 30 phút do daemon không có hạn chờ.
