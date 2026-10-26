# Test codetrace vòng 2 — JS/TS (Vue 3, React 19, Svelte 5)

Ngày 25/10/2026, ~20:00–20:51. Phiên con độc lập, chỉ đọc ba repo:

- Vue 3: `repos/easy-ai-platform-backend` (`resources/js`)
- React 19 + TSX: `/home/robertnguyen/www/yona` (alias `shared/*`, `dashboard/*`)
- Svelte 5: `repos/easy-ai-platform-widget` (`src/themes/bvdkmocchauvn`)

Công cụ: CLI `codetrace callers <đích> --compact` (thêm bản full khi cần soát), `codetrace locate`. Đối chiếu độc lập bằng `rg` và đọc code. Không sửa file nào trong repo.

## Kết luận nhanh

- **Tốc độ đạt:** ấm 1–35 ms mọi đích. Lần đầu: backend 470 ms (daemon đã chạy từ trước), yona 799 ms (`locate`), widget 1450 ms. Không lần nào phải chờ 10–60 s. Dòng `Waited:` **không xuất hiện lần nào**, chỉ có `Cost:`.
- **Đúng và đủ:** barrel `export { default as }` + alias, mảng destructure từ `createContext`, `defineExpose` + ref (báo `[call, unresolved]` trung thực), `<component :is>` qua map, `forwardRef` dạng tag, hook trả object, hook context `useNotifications`, hàm trong `.svelte.ts` (import không có đuôi `.ts`), component Svelte 29/29 dòng (cả trong `{#snippet}` và `{#each}`), alias `send as sendMessage` tách đúng khỏi hàm `send` cục bộ cùng tên.
- **Các bản sửa vòng 1:** destructure composable đã sửa (4/4 `exact`). Pinia đã sửa nhưng gắn `likely`. Nhiễu `resolve` giảm từ 132–844 dòng còn 2 dòng. **Chưa sửa:** bundle minified vẫn lọt (`public/vendor/horizon/app.js:23746`), nhiễu `layout` vẫn còn (213 dòng). Mẫu `x.prop()` khi hàm gán thuộc tính nằm lồng trong hàm khác thì **không gọi tên được** (`debounce.cancel`).
- **Lỗi nặng nhất:** symbol không tồn tại (kể cả file không tồn tại) vẫn trả về `file:1` và `Direct callers (0)`, không báo lỗi. Gõ sai tên thì kết quả trông y như code chết. Lỗi này che các chỗ bỏ sót bên dưới: hàm bọc `useCallback`, snippet Svelte, method của `useImperativeHandle`, hàm gán thuộc tính lồng nhau đều không có symbol, nên đều ra "0 người gọi".
- **Bỏ sót im lặng (grep bản full = 0):** consumer của context `provide`/`createContext`, `React.lazy`, JSX của component bọc `memo`, method `ref.current?.open()` (67 chỗ), bước gọi qua prop callback.
- **Nhiễu hệ thống mới tìm ra:** component đặt làm giá trị của khoá object (`settings: SettingsSection`, `layout: LayoutWorkspace`) khiến **mọi lần đọc `.settings`/`.layout` ở bất kỳ đâu** thành dòng unresolved (cả phép đọc, không cần gọi; cả `plugins/` và bundle vendor). 50/51 "Direct callers" của `settings-section.vue` là rác. Nhiễu này còn lan sang mọi truy vấn đi qua layout.

## Bảng kết quả

Cỡ = output `--compact` (dòng / byte). Cost = dòng `Cost:` lúc ấm.

### Vue 3 (backend)

| # | Đích | Mẫu | Kỳ vọng (rg/đọc) | codetrace | Phân loại | Cost | Cỡ |
|---|---|---|---|---|---|---|---|
| V1 | `Components/ui/sidebar/utils.js#useSidebar` | `export const [useSidebar, provideSidebarContext] = createContext()` + re-export `index.js:27` | 7 dòng: `AppSidebarFooter.vue:11` (qua barrel), `Sidebar.vue:17`, `SidebarMenuButton.vue:25`, `SidebarRail.vue:9`, `SidebarTrigger.vue:18`, `LayoutBotDetail.vue:84`, `LayoutZaloMiniAppDetail.vue:238` | 7/7 `[call]` exact | Đúng. Unresolved 222 dòng, 213 trong số đó là nhiễu `layout` (lỗi 11) | 470 ms lần đầu, 11–12 ms ấm | 422 dòng / 30,6 KB |
| V2 | `Components/ui/sidebar/SidebarProvider.vue#toggleSidebar` | `provideSidebarContext({ toggleSidebar })` → `useSidebar()` (inject) | `SidebarTrigger.vue:21` `toggleSidebar()`, `SidebarRail.vue:29` `@click="toggleSidebar"` | Chỉ có `[reference] SidebarProvider.vue:68`. **Không có** Trigger/Rail, kể cả bản full và `--tests` | **Bỏ sót im lặng** + **`exact` sai** (102 exact / 56 file: mọi nơi mount `SidebarProvider` qua cạnh reference) | 15 ms | 126 dòng / 8,5 KB |
| V3 | `Components/Bots/BotConfigVersionHistory.vue#openHistory` | `defineExpose` + ref component | `WorkspaceAgentChangeCard.vue:135` `versionHistory.value?.openHistory(...)`, template `:216` | `[reference; template] :134,:216` + `[call, unresolved] WorkspaceAgentChangeCard.vue:135` | Đúng, trung thực. 113/114 unresolved là nhiễu `layout` kế thừa | 11 ms | 74 dòng / 5,3 KB |
| V4 | `Components/ZMA/Affiliate/settings-section.vue#default` | `<component :is="pageComponent">` qua map `PAGE_COMPONENTS` | `Pages/zma/affiliate.vue:53` (map), `:60` `pageComponent`, `:7` `<component :is>` | Có `affiliate.vue:53 [reference]` → `pageComponent:60` → `default` | Đúng phần chính, **nhiễu nặng**: 105 dòng `.settings` (lỗi 11); dòng thật nằm thứ 49/51 | 15 ms | 68 dòng / 12 KB |
| V5 | `stores/modules/sidebar.js#useSidebarStore.setSelectedModule` | Pinia qua barrel `@stores` (`stores/index.js`) | `LayoutWorkspace.vue:2371` | `[call, likely] :2371` + dòng tự thân `sidebar.js:3 [callback, unresolved]` | Đúng. Nhãn `likely` hơi yếu; dòng tự thân là nhiễu (lỗi 13) | 13 ms | 13 dòng / 1,1 KB |
| V6 | `composables/useCdpColumnLabels.js#useCdpColumnLabels.save` (kiểm lại vòng 1) | destructure đổi tên `save: saveColumnLabel` | 4 dòng: `cdp/companies/index.vue:208`, `deals:519`, `profiles:355`, `tasks:185` | 4/4 `[call]` exact, đúng hàm chứa `onColumnLabel` | **Đã sửa.** Còn dòng `useCdpColumnLabels.js:67 [returned, unresolved]` thừa | 19 ms | 28 dòng / 2,3 KB |
| V7 | `stores/modules/sourceEntities.js#useSourceEntitiesStore.fetch` (kiểm lại) | Pinia `const st = useStore(); st.fetch()` | 7 dòng: `ConversationFilterMini.vue:1601,1606,1611,1616,1621`, `ConversationSidebarLeft.vue:324,332` | 7/7 `[call, likely]`. Hết 6 dòng nhiễu và hết các cạnh `exact` sai của vòng 1 | **Đã sửa.** Nhãn `likely` dù biến store khai báo ngay trong file | 11 ms | 25 dòng / 2,5 KB |
| V8 | `utils/helpers.js` `debounced.cancel = () => {}` (kiểm lại `x.prop()`) | hàm gán thuộc tính, lồng trong `debounce` | 5 dòng: `TicketFilterSearch.vue:45`, `ExternalCommentFilterSearch.vue:59`, `ConversationFilterSearch.vue:65`, `Users/FilterItem.vue:505`, `marketing/workflow/show.vue:925` | `locate helpers.js:47` → `debounce`. `#debounce.cancel` và `#debounce.debounced.cancel` → `:1`, 0 người gọi | **Bỏ sót**: không có symbol nào để hỏi | 9–10 ms | 6 dòng |
| V9 | `app.js:93` `.provide("tt", (text) => …)` | hàm ẩn danh truyền vào `provide` | ~1005 file `inject("tt")` | `locate` → `app.js#setup`. `callers app.js:94` trả về `setup` | Giới hạn: hàm ẩn danh không có symbol riêng | 8 ms | 13 dòng |

### React 19 (yona)

| # | Đích | Mẫu | Kỳ vọng | codetrace | Phân loại | Cost | Cỡ |
|---|---|---|---|---|---|---|---|
| R1 | `shared/components/charts/viz/PercentageChart.tsx#VizPercentageChart` | `export { default as VizPercentageChart }` trong `charts/index.ts` + alias `shared/*` | `dashboard/reports/components/CsatRatingDistribution.tsx:45` | 1/1 `[component]` | Đúng | 4 ms | 11 dòng / 753 B |
| R2 | `dashboard/conversation/chatlist/ConversationItem.tsx#ConversationItem` | `export default memo(ConversationItem)` | `ConversationList.tsx:131` `<ConversationItem` trong `ConversationList.renderItem` | Chỉ `(module) [reference] :175 memo(...)` → `ConversationList.tsx#(module) [import] :21`. `#default` → `:1`, 0 người gọi | **Bỏ sót** dòng JSX và hàm chứa nó; chuỗi phía trên chỉ còn đồ thị import theo module | 3–5 ms | 14 dòng / 959 B |
| R3 | `shared/people/PickPeopleDialog.tsx#PickPeopleDialog` | `forwardRef(function …)` | `GroupInfo.tsx:190`, `CreateGroupDialog.tsx:22` | 2/2 `[component]`. Thêm 2 dòng `[import]` và 1 dòng `export default` cũng tính là "Direct callers (5)" | Đúng. Nhiễu nhẹ: dòng import bị đếm là người gọi | 5 ms | 24 dòng / 2,4 KB |
| R4 | `dashboard/calls/orb/VoiceOrb.tsx#VoiceOrb`, `calls/LiveKitSession.tsx#LiveKitSession` | `lazy(() => import('./orb/VoiceOrb'))` | `CallOrb.tsx:65` `<VoiceOrb`, `CallSessionProvider.tsx:577` `<LiveKitSession` | 0 người gọi (bản full cũng 0). `CallOrb.tsx#VoiceOrb` (hằng lazy) thì thấy `:65` | **Bỏ sót im lặng**: hằng `lazy` không nối về default của module được import | 3–4 ms | 6 dòng |
| R5 | `dashboard/notifications/NotificationsContext.tsx#useNotifications` | hook bọc `useContext` | 6 dòng: `InboxEmptyState.tsx:8`, `InboxList.tsx:26`, `InboxView.tsx:18`, `InboxItemHeader.tsx:36`, `InboxListHeader.tsx:17`, `NotificationBellPanel.tsx:28` | 6/6 `[call]` exact | Đúng | 21 ms | 84 dòng / 6,9 KB |
| R6 | `dashboard/composables/useWebhooks.ts#useWebhooks.reload` | hook trả `{ reload: () => … }` rồi destructure | `Webhooks/Index.tsx:55,126,139` | 3/3 (`confirmDeletion:55`, `WebhooksIndex:126,139`) | Đúng. **Thiếu bước prop:** `Index.tsx:126 onCreated={reload}` → `NewWebHook.tsx:31 onCreated()` không hiện | 5 ms | 8 dòng |
| R7 | `NotificationsProvider.snooze`, `ConversationList.movePin`, `ConversationProvider.markMessagesRead` | `const fn = useCallback(...)` trong Provider/component, truyền qua context và prop object | snooze: `InboxItemHeader.tsx:46,48,59`. movePin: `ConversationItem.tsx:100 actions.movePin`. markMessagesRead: `MessagesView.tsx:132`, `ConversationItem.tsx:107`, `ConversationContext.tsx:679,757,784` (qua `markMessagesReadRef.current`) | snooze, movePin → `:1`, 0 người gọi. markMessagesRead → `:1`, chỉ 1 dòng `[call, unresolved] ConversationItem.tsx:107` (khớp tên) | **Bỏ sót im lặng**: hàm bọc `useCallback` không có symbol. `locate ConversationContext.tsx:821` → `ConversationProvider` | 4–35 ms | 6–10 dòng |
| R8 | `dashboard/components-next/dialog/Dialog.tsx#Dialog.open`, `PickPeopleDialog.open` | `useImperativeHandle(ref, () => ({ open }))` + `ref.current?.open()` | 67 dòng `.current?.open()`, ví dụ `GroupInfo.tsx:149` (`addRef` gắn vào `<PickPeopleDialog ref={addRef}>` :191), `GroupInfo.tsx:86,119,138` | `locate` ra đúng symbol (`Dialog.tsx:89`, `PickPeopleDialog.tsx:45`) nhưng 0 người gọi, 0 unresolved | **Bỏ sót im lặng** (Vue `defineExpose` thì bắt được dạng unresolved) | 4 ms | 6 dòng |

### Svelte 5 (widget)

| # | Đích | Mẫu | Kỳ vọng | codetrace | Phân loại | Cost | Cỡ |
|---|---|---|---|---|---|---|---|
| S1 | `src/themes/bvdkmocchauvn/lib/consent.svelte.ts#withConsent` | hàm trong `.svelte.ts`, import `"../lib/consent.svelte"` (không có `.ts`) | `Composer.svelte:45,140,165,224`, `Widget.svelte:131` | 5/5 (đúng hàm chứa `send`, `default`, `done`, `ask`) | Đúng | 1450 ms lần đầu | 19 dòng / 1,7 KB |
| S2 | `…/lib/chat.svelte.ts#send` | trùng tên với `function send()` cục bộ trong `Composer.svelte:42`, alias `send as sendMessage` | `ContactCard.svelte:38`, `chat.svelte.ts:54`, `index.ts:129`, `Widget.svelte:133` | 4/4. Không lẫn `Composer.svelte:45,56,222` (send cục bộ) | Đúng (che tên trong script) | 2 ms | 24 dòng / 2,1 KB |
| S3 | `…/components/Button.svelte#default` | component Svelte | 29 thẻ `<Button` / 13 file (`rg`) | 29/29 dòng, khớp từng dòng, kể cả trong `{#snippet actions()}` (`ContactCard.svelte:63,64`) và `{#each}` (`Suggestions.svelte:12`, `Stage.svelte:143`) | Đúng | 3 ms | 29 dòng / 4,5 KB |
| S4 | `…/components/Stage.svelte#face`, `ContactCard.svelte#actions` | `{#snippet face(a)}` → `{@render face(a)}`; snippet truyền làm prop cho `CardHead` (`{@render actions()}` :29) | `Stage.svelte:136`, `CardHead.svelte:29` | → `:1`, 0 người gọi. Lời gọi hàm bên trong snippet thì đúng (`ContactCard.svelte#slide` ← `:63,64 [template]`) | **Bỏ sót im lặng**: snippet không có symbol; ảnh hưởng hẹp (nằm trong component) | 1 ms | 6 dòng |
| S5 | `…/index.ts#start`, `#startPage` | map tên → `() => import(...)` trong `src/themes/index.js:7`, rồi `theme.start()` | `boot.js:33`, `main.js:15` | Có `boot.js:33`, `main.js:15` dạng unresolved. Kèm 9 dòng `.start()` rác | Đúng phần chính. Nhiễu: `useChatVoice.js:205,242,296`, `useVoiceAudio.js:75,117`, `URLChangeListener.js:218,234,251`, `Composer.svelte:118` | 2 ms | trong S1/S2 |
| S6 | `…/lib/recorder.ts#Recorder.start` | biến có kiểu TS trong `<script lang="ts">`: `let rec: Recorder \| undefined` (`Composer.svelte:109`), `rec = new Recorder()` (:115) | `Composer.svelte:118` | `Composer.svelte#talk [call, unresolved] :118` cùng 8 dòng `.start()` không liên quan | **Unresolved vô lý**: kiểu receiver đã biết mà vẫn unresolved | 2 ms | ~15 dòng |

Hai mẫu được yêu cầu nhưng **không có ca thật** trong các repo này (không phải tôi bỏ qua):
- `{#each}` đè tên một hàm đang truy vết: các biến lặp (`f`, `a`, `s`, `item`, `prompt`…) không trùng tên hàm nào được gọi. Đã thử ca gần nhất: che tên trong script (S2), kết quả đúng.
- `<svelte:component>`: không có trong repo. Render động trong Svelte chỉ có `<a.icon>` / `<Icon>` với icon từ lucide (bên ngoài). Đã thay bằng map tên → import động của theme (S5). `$refs.x.method()` cũng không có ca nào (chỉ `.click()`), nên Vue dùng `defineExpose` + ref (V3).

## `locate`

| Dòng | Loại | Kết quả | Đánh giá |
|---|---|---|---|
| `yona …/chatlist/ConversationList.tsx:131` | JSX trong arrow `renderItem` | `ConversationList.renderItem` (:130) | Đúng |
| `widget …/components/Stage.svelte:136` (và `:99` trong snippet, `ContactCard.svelte:61`) | template Svelte | `Stage.svelte#default` (:1) | Chấp nhận được (snippet không có symbol riêng) |
| `backend composables/useAnalysis.js:823` | hàm lồng 2 tầng | `useAnalysis.getRevenueData.buildStats` | Đúng |
| `backend Components/ui/sidebar/SidebarProvider.vue:45` | hàm trong `<script setup>` | `toggleSidebar` | Đúng |
| `yona …/ConversationContext.tsx:821` | hàm lồng bọc `useCallback` | `ConversationProvider` | **Sai/thiếu**: arrow thường thì có tên lồng (`renderItem`), bọc `useCallback` thì mất |
| `backend utils/helpers.js:47` | `debounced.cancel = () => {}` trong hàm | `debounce` | Thiếu symbol lồng |

## Danh sách lỗi

| # | Loại | Đích | Bằng chứng |
|---|---|---|---|
| 1 | **`exact` sai / nguy hiểm** | Symbol hoặc file không tồn tại | `NotificationsContext.tsx#TotallyMissing` → `:1#21a9`, "Direct callers (0)". `dashboard/notifications/Nope.tsx#X` → 0. `useCdpColumnLabels.js#useCdpColumnLabels.nope` → 0. Không có thông báo "không tìm thấy symbol" |
| 2 | Bỏ sót im lặng | `SidebarProvider.vue#toggleSidebar` (context reka-ui) | `SidebarTrigger.vue:21`, `SidebarRail.vue:29` |
| 3 | `exact` sai | như trên | 102 exact / 56 file, đi qua `[reference] SidebarProvider.vue:68` (cùng cơ chế với lỗi `return {…}` của vòng 1, nay đi qua provide) |
| 4 | Bỏ sót im lặng | `React.lazy` | `VoiceOrb.tsx#VoiceOrb` ← `CallOrb.tsx:65`; `LiveKitSession.tsx#LiveKitSession` ← `CallSessionProvider.tsx:577` |
| 5 | Bỏ sót | `memo(...)` | `ConversationItem.tsx#ConversationItem` ← `ConversationList.tsx:131` (chỉ có cạnh import `:21`; `#default` → 0) |
| 6 | Bỏ sót im lặng | hàm bọc `useCallback` | `NotificationsContext.tsx:235` snooze ← `InboxItemHeader.tsx:46,48,59`; markAllRead `:211` ← `NotificationBellPanel.tsx:55`, `InboxListHeader.tsx:36`; `ConversationList.tsx:94` movePin ← `ConversationItem.tsx:100`; `ConversationContext.tsx:820` markMessagesRead ← `MessagesView.tsx:132`, `ConversationContext.tsx:679,757,784` |
| 7 | Bỏ sót im lặng | method trong `useImperativeHandle` | `Dialog.tsx:89` / `PickPeopleDialog.tsx:45` `open` ← 67 dòng `.current?.open()`, ví dụ `GroupInfo.tsx:149` |
| 8 | Bỏ sót | bước gọi qua prop callback | `Webhooks/Index.tsx:126 onCreated={reload}` → `NewWebHook.tsx:31 onCreated()` |
| 9 | Bỏ sót im lặng | snippet Svelte | `Stage.svelte:98` face ← `:136`; `ContactCard.svelte:60` actions ← `CardHead.svelte:29` |
| 10 | Bỏ sót | hàm gán thuộc tính nằm lồng | `utils/helpers.js:46 debounced.cancel` ← `TicketFilterSearch.vue:45`, `ExternalCommentFilterSearch.vue:59`, `ConversationFilterSearch.vue:65`, `FilterItem.vue:505`, `marketing/workflow/show.vue:925` |
| 11 | **Nhiễu hệ thống** | component làm giá trị của khoá object (`key: Component`) | `settings: SettingsSection` (`affiliate.vue:53`) → 105 dòng đọc `.settings`, ví dụ `TabCustomize.vue:157`, `ConversationSetting.vue:179`, `plugins/zalo-personal-bridge/src/routes/ui-actions.mjs:1164`. `layout: LayoutWorkspace` → 106 + 107 dòng, ví dụ `DateRangePickerPanel.vue:200 props.layout === "desktop"`, **bundle `public/vendor/horizon/app.js:23746`**, `plugins/zma-template/...menu.queries.ts:13`. Đối chứng: `overview: OverviewSection` có 0 dòng nhiễu vì không ai đọc `.overview` |
| 12 | Unresolved vô lý | receiver có kiểu TS trong `.svelte` | `Composer.svelte:109,118` `rec: Recorder` → `rec.start()` unresolved |
| 13 | Nhiễu | gọi theo tên chung `.start()` | 8 dòng ở `useChatVoice.js`, `useVoiceAudio.js`, `URLChangeListener.js` gắn cho `index.ts#start` / `Recorder.start`. Không nhất quán: `.open()` (67 dòng) thì không ra ứng viên nào |
| 14 | Nhiễu nhẹ | dòng tự thân trong Direct callers | `useCdpColumnLabels.js:67 [returned, unresolved]`, `sourceEntities.js:5` / `sidebar.js:3 [callback, unresolved]` |
| 15 | Nhãn yếu | Pinia | `LayoutWorkspace.vue:2371`, `ConversationFilterMini.vue:1601` là `likely`, dù biến store khai báo ngay trong file |
| 16 | UX | con trỏ unresolved ở compact | "listed by callers of 'LayoutWorkspace.vue#default' with depth 1": chạy lại lệnh đó với `--compact` chỉ in lại con trỏ. Danh sách dòng chỉ có ở bản full |
| 17 | Nhiễu output | cảnh báo không liên quan | Truy vấn JS ở backend luôn in cảnh báo PHP `tests/Feature/CRM/BusinessObjectPaymentTest.php`. Widget in 2 cảnh báo SFC rỗng (`ChatSidebar.vue`, `IconChatAskAI.vue`). Yona tính dòng `[import]` là "Direct callers" (R3: 5 mục, 2 mục thật) |

## Kiểm lại các bản sửa vòng 1

| Bản sửa | Kết quả |
|---|---|
| Destructure kết quả composable (`save: s`) | Đã sửa (V6 4/4 exact) |
| Pinia `st.fetch()` | Đã sửa, nhãn `likely` (V7 7/7, V5) |
| Gọi `x.prop()` sau khi gán hàm vào thuộc tính | Chưa kiểm được ca thật: hàm gán nằm lồng trong hàm khác nên không có symbol (lỗi 10) |
| Nhiễu `path.resolve`/`Promise.resolve` | Giảm mạnh: còn `app.js:24` và `ssr_unused.js:8` ở mọi truy vấn backend |
| Bundle minified | Chưa sạch hẳn: `public/vendor/horizon/app.js:23746` vẫn lọt qua nhiễu `layout` |

Status: DONE_WITH_CONCERNS
