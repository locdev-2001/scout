# Test codetrace trên JS/TS/Vue — backend `resources/js`

Ngày 25/10/2026, 18:56–19:43. Checkout chính `repos/easy-ai-platform-backend` (chỉ đọc, không sửa). Công cụ: MCP `codetrace` (`callers` compact mặc định, `locate`) và CLI `codetrace` để đo thời gian và lấy bản full. Đối chiếu bằng `rg` và đọc code.

## Kết luận nhanh

- Tìm chỗ gọi trực tiếp của một hàm import, một component dùng dạng tag (kể cả tự import, không có dòng import) và method Options API: **đúng và đủ** (8/8, 57/57, 2/2). `locate` đúng 7/7 dòng thử.
- **Lỗi hệ thống 1 (bỏ sót):** hàm trả về từ composable (`return { save }`) rồi được destructure ở nơi dùng: tool dừng ở dòng `return`, **không bao giờ chỉ ra dòng gọi thật** hay hàm chứa nó, dù có đổi tên khi destructure hay không.
- **Lỗi hệ thống 2 (`exact` sai):** cạnh `[reference]` ở `return {...}` của composable và `[callback]` ở `defineStore(...)` biến cả composable/store thành "người gọi". Mọi chỗ gọi `useAnalysis()` / `useSourceEntitiesStore()` thừa hưởng nhãn `exact`, kể cả khi không dùng gì liên quan. Vùng ảnh hưởng phình to nhiều lần.
- **Lỗi hệ thống 3 (unresolved rác):** các symbol tên chung ở phía trên chuỗi (`app.js#resolve`, `ssr_unused.js#resolve`, `layout`, `register`) bị khớp theo tên với `path.resolve`, `Promise.resolve`, `props.layout`, bundle vendor đã minify. 844/844 dòng unresolved của đích 1 là rác.
- **Ổn định:** lần gọi CLI đầu **treo 1788 s (~30 phút)**, MCP gặp `Connection reset by peer` đúng lúc daemon tự tắt, có một tiến trình zombie. Sau khi ấm: 10–47 ms server, CLI 0,02 s wall, đạt mục tiêu dưới 100 ms.

## Bảng kết quả

| # | Đích | Lý do chọn | Chỗ dùng (rg/đọc tay) | codetrace | Bỏ sót | `exact` sai | Unresolved thừa | Cost MCP | Compact | Full |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `utils/currencyValue.js#normalizeCurrencyValue` | Hàm utils dùng ở 4 composable, **mọi chỗ import qua re-export** `utils/helpers.js:10` | 8 dòng gọi / 4 file | Direct 8/8 đúng; "Further up" 110 symbol / 79 file; 39 entry | Hàm chứa dòng gọi thật ở tầng trên (vd. `Countries.vue#fetchData`) | Có: ít nhất 4 component chứng minh được (bên dưới) | 844 dòng, toàn rác | 180 ms (lần đầu), sau đó 23 ms | 144 dòng / 11,6 KB | 1148 dòng / 225 KB |
| 2 | `composables/useCdpColumnLabels.js#useCdpColumnLabels.save` | Composable và hàm trả về, destructure có đổi tên `save: saveColumnLabel` | 4 dòng gọi / 4 trang | Direct chỉ có `return {…}` :67; trang chỉ có `#default` qua dòng destructure | **4/4 dòng gọi thật** và hàm `onColumnLabel` | Không (đúng file nhưng sai lý do) | 132 dòng (`resolve`) | 47 ms | 24 dòng / 1,5 KB | 157 dòng / 28 KB |
| 3 | `Components/CDP/CdpContainer.vue#default` | Component dùng dạng tag; 26 file import tường minh, phần còn lại tự import qua `unplugin-vue-components` | 57 tag / 52 file (PascalCase; kebab-case 0) | 57/57 dòng, khớp từng dòng | 0 | 0 | 132 dòng (`resolve`) | 12 ms | 119 dòng / 12,9 KB | — |
| 4 | `Pages/auth/register.vue#validateName` | Method Options API, gọi từ template `@blur` và `this.validateName()` | 2 | 2/2 (`[template]` :44, `[call]` :432) | 0 | 0 | 178 dòng (`register` 46 + `resolve` 132) | 10 ms | 19 dòng / 1,3 KB | — |
| 5 | `stores/modules/sourceEntities.js#useSourceEntitiesStore.fetch` | Action Pinia, trùng tên `fetch` toàn cục | 7 dòng gọi / 2 file | 7/7 có mặt nhưng **đều `unresolved`** + 6 dòng không liên quan | 0 | Có: ConversationItem, ChatArea, SidebarRight, ZaloPersonalGroupActions… | 151 dòng | 13 ms | 51 dòng / 7,0 KB | 430 dòng / 95 KB |

Cột "Compact" đo bằng CLI `--compact` (cùng nội dung với MCP mặc định).

## Chi tiết và bằng chứng

### Đích 1 — `normalizeCurrencyValue` (re-export)

- 8 chỗ gọi theo `rg`: `useAnalysis.js:452,549,679,829`, `useCdpProfileHighlights.js:63`, `useProfileEventsActivity.js:291`, `useProfileEventsTimeline.js:267,736`. Đều import từ `@/utils/helpers`, file này re-export ở `utils/helpers.js:10`. codetrace trả đủ 8 dòng, tên hàm chứa đúng (vd. `useAnalysis.getRevenueData.buildStats`). Re-export được xử lý đúng.
- **`exact` sai:** chuỗi là `normalizeCurrencyValue ← useAnalysis.parseCurrency ← useAnalysis [reference] useAnalysis.js:1380 return {… parseCurrency}`, rồi mọi chỗ gọi `useAnalysis()` đều được gắn `[call]` exact. Bản full có `Weekdays.vue#default [call] Weekdays.vue:156 useAnalysis();`. Chứng minh là sai:
  - `Components/Analytics/Weekdays.vue:155-156` chỉ lấy `getOverviewBucketed, buildTimeParams, buildFilterParams`. Thân `getOverviewBucketed` (`useAnalysis.js:214-220`) chỉ gọi `callMetricsApi`.
  - `Realtime/LiveCountUser.vue:11` chỉ lấy `getRealTimeUsers` (`useAnalysis.js:529-531`, chỉ `callMetricsApi`).
  - `EventSelector.vue:98` chỉ lấy `getEventNames` (`useAnalysis.js:257-259`).
  - `Realtime/RealtimeGlobe.vue:30` chỉ lấy `getRealtimeSessionLocations` (`useAnalysis.js:533-535`).

  Cả 4 component và các trang chứa chúng (vd. `Pages/analytics/realtime.vue`) bị liệt kê là vùng ảnh hưởng `exact` của một hàm chuẩn hoá tiền tệ mà chúng không bao giờ chạy tới. "39 entry points" bị thổi phồng.
- **Bỏ sót ở tầng trên:** `Countries.vue` thật sự gọi `getItemRevenueData` ở `:285,298,311,324` trong hàm `fetchData` (`locate Countries.vue:285` → `Countries.vue#fetchData`). Output chỉ ghi `Countries.vue: default`, không có `fetchData` và không có các dòng này. Kiểm thêm `callers useAnalysis.getItemRevenueData`: direct caller duy nhất là `useAnalysis [reference] :1376`, 4 dòng gọi ở Countries.vue không xuất hiện.
- **Unresolved 844 dòng:** 7 nhóm, `app.js#resolve` và `ssr_unused.js#resolve` mỗi nhóm 132 dòng, 5 trang `zma/affiliate*.vue#layout` mỗi trang 116 dòng. Xem `--depth 1`: toàn `path.resolve(...)` trong `.opencode/…`, `Promise.resolve()` trong `plugins/shopee-seller-bridge/src/index.mjs:656`, và các lượt đọc thuộc tính `props.layout === "desktop"` (`DateRangePickerPanel.vue:200`), `body.layout?.enabled` (`menu-footer-tab.vue:541`), `public/vendor/horizon/app.js:23746`. Không có dòng nào gọi được `resolve` của Inertia hay option `layout`.
- `Hidden: 227 callers inside tests`, nhưng `--tests` chỉ thêm 1 file test thật (`tests/JavaScript/Helpers/normalizeCurrencyValue.test.js`, 10 dòng). 217 còn lại là unresolved rác theo tên. Con số dễ gây hiểu lầm.

### Đích 2 — `useCdpColumnLabels` và `save` trả về

- `callers useCdpColumnLabels`: 4/4 đúng (`cdp/{companies,deals,profiles,tasks}/index.vue`, dòng destructure :202, :513, :334, :179).
- `callers useCdpColumnLabels.save`: direct caller duy nhất là `useCdpColumnLabels [reference] useCdpColumnLabels.js:67 return { labels, apply, save };`. **4 dòng gọi thật bị bỏ sót**, không có trong cả compact lẫn full (`grep saveColumnLabel` trên bản full chỉ ra 4 dòng destructure):
  - `Pages/cdp/tasks/index.vue:185`
  - `Pages/cdp/deals/index.vue:519`
  - `Pages/cdp/companies/index.vue:208`
  - `Pages/cdp/profiles/index.vue:355`

  Hàm chứa chúng là `onColumnLabel` (`locate tasks/index.vue:185` → `#onColumnLabel :184`; `locate deals/index.vue:519` → `#onColumnLabel :518`). Hàm này không có trong vùng ảnh hưởng, nên agent không biết handler `@column-label="onColumnLabel"` (`tasks/index.vue:22`) bị đụng tới.
- `apply` (`applyColumnLabels(...)` ở `tasks:183`, `deals:517`, `companies:206`, `profiles:345`) bị bỏ sót y hệt. Đích 1 cho thấy lỗi xảy ra cả khi **không đổi tên** (`getItemRevenueData`). Kết luận: tool không nối được từ biến đã destructure từ kết quả composable tới thuộc tính trả về. Đây là mẫu phổ biến nhất của Vue Composition API.
- File-level vẫn đúng (4 trang), nhưng chỉ vì cùng các trang đó gọi `useCdpColumnLabels`, chứ không phải vì tool thấy lời gọi `save`.

### Đích 3 — `CdpContainer.vue` (component tự import)

- `rg "<CdpContainer[ >/]|<CdpContainer$|<cdp-container…"`: 57 dòng / 52 file. So từng dòng bằng script: codetrace 57, rg 57, **thiếu 0, thừa 0**. Có 26 file import tường minh, 26 file dùng tag không có dòng import (vd. `Pages/call-center/ai/audio/index.vue:50`, `Pages/call-center/ai/campaigns/index.vue:16`): tự import được bắt đủ. Không có cách dùng `:is=`/`h(CdpContainer)`. Kebab-case không có trong repo, nên chưa kiểm được nhánh này.
- Nhiễu: "Affected entry points (52)" lặp lại gần như y nguyên 52 dòng direct (tất cả đều là trang Inertia), làm compact dài gấp đôi. "Further up" có `ssr_unused.js` nhưng không có `app.js` (đích 1 có cả hai), không nhất quán. `Hidden: 6 callers inside tests`, nhưng `--tests` không thêm dòng nào trong `tests/`; 6 con số này là `Promise.resolve()` trong `tests/JavaScript/…` khớp tên `resolve`.

### Đích 4 — method `validateName` (Options API)

- `rg`: `register.vue:44 @blur="validateName"` và `:432 this.validateName();`. codetrace trả đúng 2/2, kèm loại `[template]`/`[call]`.
- Unresolved thừa: `register.vue#register: 46 lines in 4 files`. `--depth 1` cho thấy 44 dòng nằm trong `resources/js/lib/ckeditor5/build/ckeditor.js` (minified) và `public/vendor/{horizon,log-viewer}/app.js`, cộng `navigator.serviceWorker.register` (`useFCM.js:140`). Method của một SFC Options API chỉ gọi được qua `this`/template/ref, nên bundle vendor không thể gọi nó. Các file vendor này được git theo dõi (`git check-ignore` không khớp), nên đây là thiếu bước loại trừ bundle vendor/đã build, không phải lỗi `.gitignore`.

### Đích 5 — action Pinia `useSourceEntitiesStore.fetch`

- `rg "sourceEntitiesStore\.fetch"`: 7 dòng, `ConversationFilterMini.vue:1601,1606,1611,1616,1621` và `ConversationSidebarLeft.vue:324,332`. codetrace có đủ 7, tên hàm chứa đúng (`onMessengerSubOpen`…), nhưng **đều gắn `unresolved`** dù trong cùng file có `const sourceEntitiesStore = useSourceEntitiesStore()` (vd. `ConversationFilterMini.vue:704`). README có ghi giới hạn này, nhưng với mẫu Pinia chuẩn thì kiểu suy ra được ngay trong file.
- Không bị lẫn với `fetch()` toàn cục, tốt. Có 6 dòng thừa: `window.fetch.bind` (`plugins/shopee-seller-bridge/src/index.mjs:4872`), `dealStageFilterStore.fetch` (`ConversationFilterDealStage.vue:77`, `ConversationFilterMini.vue:1587`), `dealStages.fetch` (`DealStageChangedTriggerConfig.vue:270`), `cursorGrid.fetch` (`useCdpList.js:137`), `grid.fetch` (`useRelationRecords.js:35`). Biến `dealStageFilterStore` là store khác đã biết nguồn, nên có thể loại.
- **`exact` sai:** `fetch ← useSourceEntitiesStore [callback] sourceEntities.js:5 defineStore(...)`, rồi mọi nơi gọi `useSourceEntitiesStore()` và mọi lượt đọc biến store đều thành exact. Bản full có `ConversationItem.vue#sourceContextName [reference] :433 return sourceEntitiesStore.hasMultipleMessenger;`. Thực tế:
  - `ConversationItem.vue:433-441,459` chỉ đọc getter `hasMultiple*` và gọi `getSourceEntity`.
  - `ConversationChatArea.vue:198` và `ConversationSidebarRight.vue:291` chỉ gọi `getSourceEntity`.
  - `ConversationZaloPersonalGroupActions.vue:605` và `stores/modules/zaloPersonalFriendRequests.js:101` chỉ đọc `.zalo_personal`.

  Những file này không gọi `fetch`, nhưng đóng góp phần lớn "76 symbols in 16 files" exact.
- Dòng `Unresolved… useSourceEntitiesStore.fetch: 14 lines in 7 files` trỏ ngược về chính đích, lặp lại phần direct.

### `locate`

| Dòng | Ngữ cảnh | Kết quả | Đúng? |
|---|---|---|---|
| `Pages/cdp/tasks/index.vue:185` | `<script setup>`, trong arrow `onColumnLabel` | `#onColumnLabel :184` | Đúng |
| `Pages/cdp/tasks/index.vue:16` | template `<CdpContainer layout="list">` | `#default :1` | Đúng |
| `composables/useAnalysis.js:829` | `.js`, hàm lồng 2 cấp | `#useAnalysis.getRevenueData.buildStats :821` | Đúng (`:821` là `const buildStats =`) |
| `Pages/auth/register.vue:432` | Options API `methods` | `#register :427` | Đúng |
| `composables/useCdpColumnLabels.js:2` | dòng import | `#(module) :1` | Đúng |
| `Components/Analytics/Countries.vue:285` | trong `fetchData` | `#fetchData :264` | Đúng |
| `Pages/cdp/deals/index.vue:519` | arrow trong script setup | `#onColumnLabel :518` | Đúng |

`callers tasks/index.vue:185` → `onColumnLabel ← #default [template] :22 @column-label="onColumnLabel"`: đúng.

### Thời gian và độ ổn định

- MCP: lần đầu 180 ms (daemon có vẻ đã được phiên khác làm ấm), các lần sau 10–47 ms. CLI sau khi ấm: 0,02 s wall cho cả 5 đích. Đạt mục tiêu.
- **Treo:** lệnh CLI đầu tiên `codetrace callers '…#normalizeCurrencyValue' --compact` chạy khoảng 18:57:40 và mất **1788,87 s wall**. Daemon backend lúc đó (pid 1109785, khởi động 18:57:02, con của `codetrace mcp` pid 1107448) thoát lúc ~19:27, đúng mốc 30 phút "tự tắt khi không ai hỏi". Giả thuyết: yêu cầu của CLI không bao giờ được phục vụ và cũng không làm mới bộ đếm nhàn rỗi. Sau đó pid 1109785 thành `[codetrace] <defunct>` (MCP không reap). Daemon mới khởi động 19:27:32.
- Cùng lúc đó, lệnh MCP `callers useCdpColumnLabels.save` trả `Connection reset by peer (os error 104)`; gọi lại thì được.
- Hai lần CLI kế tiếp mất **10,86 s và 18,40 s wall**, trong khi dòng `Cost:` in **23 ms**. `Cost:` chỉ đo phần server, không tính thời gian chờ daemon/driver khởi động, nên không phản ánh độ trễ người gọi phải chịu.
- Lưu ý: hai phiên anh em đang build/chạy daemon codetrace song song (rust-analyzer ~113% CPU, có daemon trên bản sao `backend-copy`). Máy bận có thể góp phần làm chậm, nhưng zombie, connection reset và `Cost:` sai lệch là của tool. Bộ nhớ thường trú 895 MB → 1047 MB (server + driver).

### Output gọn có đủ và có quá dài?

- **Đủ ở tầng trực tiếp** khi tool resolve được (đích 1, 3, 4): có dòng code, tên hàm chứa và hash, agent dùng ngay được.
- **Không đủ** với mẫu composable/store (đích 2, 5): không có dòng gọi thật hoặc gắn unresolved, trong khi vùng "Further up" lại thổi phồng bằng exact sai. Agent sẽ hoặc đọc thừa rất nhiều (đích 1: 79 file), hoặc bỏ qua handler thật (`onColumnLabel`).
- **Dài vì nhiễu, không vì nội dung.** Mỗi truy vấn JS đều có:
  - dòng `Warning: tests/Feature/CRM/BusinessObjectPaymentTest.php could not be analysed…`, một lỗi PHP không liên quan;
  - 1–7 dòng "Unresolved … `resolve`/`layout`" kèm con số 132–844 khiến agent tưởng phải đọc tay hàng trăm dòng;
  - với component/trang, "Affected entry points" lặp lại danh sách direct.

  Bỏ ba thứ đó, đích 3 còn khoảng 60 dòng thay vì 119, đích 1 còn khoảng 100 thay vì 144.

## Đề xuất sửa (theo mức ảnh hưởng)

1. Nối thuộc tính trả về của composable tới chỗ dùng: `const { a, b: c } = useX()` rồi `a()`/`c()` → caller thật của `useX.a`/`useX.b`, đúng dòng và đúng hàm chứa. Áp dụng tương tự cho `const s = useXStore(); s.fetch()` (Pinia), cho kết quả `exact` hoặc ít nhất `likely`.
2. Bỏ cạnh `[reference]` ở `return {…}` và `[callback]` ở `defineStore(...)` như cạnh "gọi". Nếu muốn giữ để an toàn, gắn `likely`/`unresolved` thay vì `exact`, và không lan tiếp sang mọi caller của factory.
3. Dừng ở điểm vào: `app.js#resolve` (Inertia `resolve`) và option `layout` của `defineOptions` không cần tìm caller theo tên. Không khớp tên với lượt đọc thuộc tính (`props.layout`) hay với receiver đã biết (`path.resolve`, `Promise.resolve`, `navigator.serviceWorker.register`, `window.fetch`).
4. Loại bundle vendor/đã build khỏi khớp tên: `public/vendor/**`, `resources/js/lib/**/build/**`. Cân nhắc loại cả `.opencode/**` và `plugins/**` khi đích nằm trong `resources/js`.
5. Đếm `Hidden: N callers inside tests` riêng exact và unresolved. Không in warning PHP trong truy vấn JS. Không lặp entry point khi trùng direct.
6. Sửa treo: CLI không được chờ vô hạn một daemon không trả lời (cần timeout và khởi động lại); MCP phải reap tiến trình con; `Cost:` nên gồm cả thời gian chờ phía client.

Status: DONE_WITH_CONCERNS
Tóm tắt: Tìm chỗ gọi trực tiếp (re-export, component tự import, method Options API) và `locate` đều chính xác, nhanh dưới 50 ms khi ấm. Có ba lỗi hệ thống: bỏ sót mọi lời gọi hàm destructure từ composable, gắn `exact` sai cho mọi người dùng composable/store, và unresolved rác theo tên chung. Ngoài ra lần gọi CLI đầu treo 30 phút.
