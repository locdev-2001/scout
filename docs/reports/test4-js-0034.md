# Test codetrace vòng 4, đợt B: JS/TS/Vue/React/Svelte so với rg

Ngày 26/10/2026, khoảng 00:00–00:34. Phiên con độc lập, chỉ đọc. Không sửa repo nào, không sửa `/home/robertnguyen/www/codetrace`. Riêng `yona/refs/claude-code` có thêm một bản sao trong scratchpad (thêm `package.json`, sửa specifier) để cô lập nguyên nhân; số liệu chính vẫn lấy trên repo thật.

Cách đo: `codetrace callers <đích> --depth 1 --compact`, thời gian thực đo bằng `date +%s%N` quanh lệnh, kèm dòng `Cost:`. "Sự thật" lấy bằng `rg -n '\bten\b'` rồi đọc từng dòng; với đích nhiều dòng, so tập `file:dòng` bằng script (`comm`).

## Tóm tắt

50 đích trên 6 repo: **hơn rg 16, ngang 21, kém 13**.

Trên repo một gói, codetrace thật sự tốt hơn rg: tách được hàm trùng tên (`matches`, `formatPrice`, `delay`), bắt được component qua barrel + alias `@` (kể cả khi không có `node_modules`), bắt hàm truyền làm callback (`setTimeout(hideToast)`, `.map(columnName)`, `MutationObserver(scheduleRefresh)`), closure lồng (`register.redraw` 14/14), và mỗi lần hỏi ấm chỉ 1–5 ms. Lỗi "tên không tồn tại vẫn ra `file:1`" của vòng 2 đã sửa: giờ ra `Not found`.

Nhưng có ba lỗ **bỏ sót im lặng** rất lớn, đều ra "Direct callers (0)" như code chết:

1. **Monorepo workspace** (`@debtnote/money-core`, `@oh-my-pi/pi-utils`): import theo tên gói không được phân giải, nên mọi lời gọi xuyên gói biến mất. Ở oh-my-pi là 18.117 dòng import; `untilAborted` có 320 dòng gọi, codetrace báo 0.
2. **Specifier `.js`/`.jsx` trỏ tới file `.ts`/`.tsx`** (chuẩn ESM của TypeScript): không phân giải. claude-code có 1.011 + 21 import kiểu này.
3. **Repo TS không có `package.json` ở gốc** (claude-code): driver JS không bật, chỉ còn bộ đếm theo chữ. `path#name`, `path:line` và `locate` đều ra `Not found`; tên trần thì ra kết quả kiểu rg, nhãn `unresolved`.

Thêm hai lỗi nặng về độ đúng và tài nguyên:

4. **Số dòng trong file Vue lệch** (lệch thêm 1–8 dòng) khi phần template phía trên có thuộc tính viết trên nhiều dòng: `locate` trả sai hàm, dòng code in ra là `</div>` hay dòng trống. 13/136 file `.vue` trong widget bị ảnh hưởng.
5. **oh-my-pi:** câu hỏi TS kéo theo `rust-analyzer` (6–7 GB RSS). Lần hỏi đầu quá 120 s không trả lời. Sau đó cứ vài câu lại có một câu mất 20–21 s, kèm "a driver restarted"; lúc đó RAM trống chỉ còn 263–412 MB.

## Thời gian lần đầu và các lần sau

| Repo | File | Lần đầu (thực) | Ấm (Cost) | RAM báo |
|---|---|---|---|---|
| debtnote | ~115 TS/TSX | 728 ms | 1–3 ms | 63–75 MB |
| easy-ai-extension (không có `node_modules`) | ~112 TS | 766 ms | 1–2 ms | 62–76 MB |
| jam-easychatgpt-widget (symlink tới `easy-ai-platform-widget`) | ~300 Vue/JS/TS/Svelte | 1.625 ms | 1–5 ms | 67–79 MB |
| metea | ~320 Vue/TS | 1.432 ms | 2–12 ms | 177–196 MB |
| claude-code (thật, chạy driver generic) | ~1.370 TS | 159 ms | 1–2 ms | 25 MB |
| claude-code (bản sao có `package.json`) | như trên | 858 ms | 2–4 ms | 125–141 MB |
| oh-my-pi | ~6.100 TS + crate Rust | **>120 s, không có output** (`timeout` rc=124) | 11–160 ms, hoặc **20–21 s** khi driver khởi động lại | 7,9 GB (gần hết là rust-analyzer) |

Mỗi câu ấm còn tốn thêm ~17–20 ms để khởi động CLI. `--compact` cho output nhỏ: 0,3–6,5 KB với `--depth 1`; 19 KB với `cn` (120 dòng gọi).

Truy vấn không giới hạn `--depth` (bọc `timeout 120`): debtnote `formatVnd` 21 ms (39 dòng, 3,7 KB); extension `matches` 21 ms; widget `toMoney` 26 ms (60 dòng), `delay` 24 ms (177 dòng, 27 KB); metea `cn` 34 ms (191 dòng, 24 KB); bản sao claude-code `baseNameOf` 24 ms. Ở oh-my-pi tôi không chạy truy vấn không giới hạn: chỉ một câu `--depth 1` đã đẩy RAM xuống sát mức cạn.

## Bảng từng đích

"rg đúng/sai" = số dòng gọi thật / số dòng rg trả về nhưng không phải lời gọi (import, định nghĩa, chú thích, tên khác). "ct" = codetrace: đúng/sót/thừa. Không ghi chú gì thêm nghĩa là nhãn hợp lý.

### 1. debtnote (pnpm monorepo, React)

| # | Đích | rg đúng/sai | ct đúng/sót/thừa | Nhãn | Kết luận |
|---|---|---|---|---|---|
| D1 | `packages/money-core/src/index.ts#calculateOutstandingAmount` (import `@debtnote/money-core`) | 5 (+4 test) / 4 | **0/5/0** | — | **Kém**: không đọc import kiểu workspace (`ledger-service.ts:58,88,97`, `payment-request.ts:52`, `ledger.ts:32`) |
| D2 | `apps/web/src/components/money/format-vnd.ts#formatVnd` | 12 / 15 | 12/0/0 | exact | Hơn: ra đúng hàm chứa, bỏ import và test |
| D3 | `money-amount.tsx#MoneyAmount` (JSX) | 22 / 9 | 22/0/0 | exact `[component]` | Hơn |
| D4 | `ledger-service.ts#LedgerService.revokeShare` (`ctx.services.ledger.revokeShare`) | 1 / 1 | 1/0/0 | unresolved | Ngang |
| D5 | `use-ledger.ts#useLedger` | 1 / 2 | 1/0/0 | exact | Ngang |
| D6 | `ledger-service.ts#projectLedger` | 2 / 0 | 2/0/0 | exact | Ngang |
| D7 | `tesseract-receipt-reader.ts#createTesseractReceiptReader.read` (method của object trả về, kiểu `ReceiptReader`) | 2 (`fallback-receipt-reader.ts:10`, `receipt-worker.ts:22`) | **0/2/0** | — | **Kém.** Ngược lại, `PostgresLedgerRepository.read` lại nhận đúng 2 dòng `reader.read` đó làm unresolved (sai) |
| D8 | `ledger-repository.ts#columnName` (`.map(columnName)`) | 4 / 0 | 4/0/0 | exact `[call; reference]` | Hơn (tách gọi và truyền) |
| D9 | `owner-app.tsx#OwnerApp` (`lazy(() => import(...).then(m => ({default: m.OwnerApp})))`) | 1 / 0 | 1/0/0 | unresolved | Ngang. Chuỗi dừng ở nút unresolved, kể cả `--depth 3` |
| D10 | `calendar-picker.tsx#default` (`lazy(() => import('./calendar-picker'))`) | 1 | 1/0/0 | `[lazy import]` exact | Hơn. Nhưng hỏi tiếp const `date-picker-field.tsx#CalendarPicker` thì kéo thêm 3 dòng `import { DatePickerField }` làm người gọi exact (lỗi 7) |

### 2. easy-ai-extension (WXT + React, không có `node_modules`, alias `@/*` lấy từ tsconfig)

| # | Đích | rg đúng/sai | ct | Nhãn | Kết luận |
|---|---|---|---|---|---|
| E1 | `instagram.ts#instagramAdapter.uploadMedia` | 1 / 0 | 1/0/0 | unresolved | Ngang |
| E2 | `src/ui/Button.tsx#Button` (barrel `@/src/ui`) | 5 / 2 | 5/0/0 | exact | Hơn |
| E3 | `registry.ts#platformFor` | 1 / 2 | 1/0/0 | exact | Ngang |
| E4 | `facebook.ts#facebookAdapter.account` | 1 / 0 | 1/0/0 | unresolved | Ngang |
| E5 | `toast.ts#hideToast` (có `setTimeout(hideToast)`) | 4 / 2 | 4/0/0 | exact | Hơn |
| E6 | `filter-layer.ts#matches` (tên phổ biến) | 2 / 23 | 2/0/0 | exact | **Hơn rõ** |
| E7 | `filter-layer.ts#setFilter` | 9 / 3 | 9/0/0 | exact | Hơn |
| E8 | `list-rows.ts#scheduleRefresh` (truyền làm callback) | 5 / 2 | 5/0/0 | `[call; reference]` | Hơn |
| E9 | `device.ts#heartbeatDevice` | 4 (+6 test) / 2 | 4/0/0 | exact | Hơn |
| E10 | `zalo-tab.ts#installZaloTab` (`defineContentScript({ main: installZaloTab })`) | 1 / 1 | 1/0/0 | `[handed, unresolved]` | Ngang (đáng ra exact) |

### 3. jam-easychatgpt-widget (Vue 3.4.21 + Svelte)

| # | Đích | rg đúng/sai | ct | Nhãn | Kết luận |
|---|---|---|---|---|---|
| J1 | `src/utils/toMoney.js#default` (barrel `@utils`, `../../utils`, `@utils/toMoney`) | 45 / ~19 | 45/0/0, nhưng **8 dòng sai số** | exact | Ngang: đủ như rg và tự bỏ import, nhưng `CardProductOrder.vue:41` (thật 40), `CardProductRowNew.vue:299,323` (thật 297,319), `CardProductNew.vue:319` (thật 314), `FlowCheckout.vue:997…1043` (thật 991…1037), đoạn code in ra là `</div>` |
| J2 | `stores/modules/toast.js#actions.error` (`toastStore.actions.error`) | 9 / 0 (với `\.actions\.error\(`) | 9/0/**15** | toàn unresolved | **Kém**: thừa 15 dòng đọc `.error` (`props.error`, `data?.error`, `event.error`); `FlowSelectVariants.vue:411` thật ra là 410 |
| J3 | `useCart.js#useCart.update` (`useCart().update(...)`) | 4 / 0 | 4/0/1 | exact + 1 dòng tự thân `[returned, unresolved]` | Ngang. `CardProductOrder.vue:88,102` thật ra là 87,101 |
| J4 | `utils/delay.js#default` | 11 / ~40 | 11/0/0 | exact | **Hơn rõ** |
| J5 | `TypingIndicator.vue#default` (`defineAsyncComponent({ loader: () => import(...) })`) | 1 | 1/0/0 | `[lazy import]` | Ngang |
| J6 | `Slider.vue#scrollTo` (`defineExpose` + slot `v-bind="{ scrollTo }"`) | 4 | 3/1/4 | thừa: DOM `.scrollTo` | Kém nhẹ: sót `ChatView.vue:964` (gọi qua slot scope) |
| J7 | `Slider.vue#scrollNext` (template `sliderRef?.scrollNext()`) | 1 | **0/1/2** | thừa: `emblaApi.value?.scrollNext()` | **Kém**: sót `RCM/WidgetComboHorizontal.vue:123`, dù vòng 2 bắt được dạng `ref.value?.x()` trong script |

### 4. metea (Laravel + Vue 3 + Pinia + MDX)

| # | Đích | rg đúng/sai | ct | Nhãn | Kết luận |
|---|---|---|---|---|---|
| M1 | `stores/content.ts#useContentStore.rowsOf` | 3 / 2 | 3/0/1 (tự thân `:515`) | exact | Hơn |
| M2 | `useContentStore.load` (`useContentStore().load`, `content.load`) | 5 / 0 | 5/0/1 (tự thân) | exact | Hơn (ra đúng hàm chứa) |
| M3 | `useFormatPrice.ts#useFormatPrice` | 2 / 3 | 2/0/0 | exact | Ngang |
| M4 | `cms/lib/format.ts#formatPrice` (trùng tên với biến `formatPrice = useFormatPrice()`) | 3 / 9 | 3/0/0 | exact | **Hơn rõ** |
| M5 | `GuideCallout.vue#default` (dùng trong 19 file `.mdx` qua `MDXProvider`) | 19 | 1 dòng nối `mdxComponents`, **19 dòng `.mdx` không thấy** | exact | Kém: `.mdx` không được đọc, cũng không có cảnh báo |
| M6 | `views/ProductDetailView.vue#default` (route lazy) | 1 | 1/0/0 | `[lazy import]` | Ngang |
| M7 | `components/ui/button/Button.vue#default` (barrel `export { default as Button }`) | 100 | 101/0/0 (thêm `cms/lib/columns.ts:56` `h(Button)`) | exact | Hơn |
| M8 | `components/ui/button/index.ts#buttonVariants` (`export const = cva(...)`) | 4 | 4/0/**31** | thừa 31 dòng `import { Button }` gắn exact | **Kém** (lỗi 7) |
| M9 | `lib/utils.ts#cn` | 120 | 120/0/0 | exact | Hơn (khớp từng dòng) |

### 5. yona/refs/claude-code (TS, không có `package.json`)

Repo thật, chạy driver generic:

| # | Đích | rg | ct | Kết luận |
|---|---|---|---|---|
| C1 | `mods/diff/hooks/classify/is-noise-file.ts#isNoiseFile` (cả dạng `:11` và `locate`) | 4 | **`Not found`** | Kém |
| C2 | `baseNameOf` (tên trần) | 2 (+3 dòng khác) | 2, `[text call, unresolved]`, không có tên hàm chứa | Ngang |
| C3 | `arrayOf` | 3 | 3 | Ngang |
| C4 | `hunkAfter` | 2 | 2 | Ngang |
| C5 | `gitBackendOf` | 1 | 1 | Ngang |
| C6 | `titleRow` | 1 | 1 | Ngang |
| C7 | `register` (4 hàm trùng tên trong 4 mod) | 165 dòng | 69 dòng, lẫn mọi mod, **tính cả định nghĩa method `register(on) {` là "text call"** | Kém |
| C8 | `partitionOf` | 2 (+4 test) | 6, không giấu test | Ngang |

Bản sao có thêm `package.json`: driver JS bật, nhưng `isNoiseFile` và `baseNameOf` vẫn ra 0 người gọi, kể cả với `--tests`. Đổi `'./is-noise-file.js'` thành `'./is-noise-file'` trong bản sao thì ra ngay 2/2 exact. Bỏ hết đuôi `.js`/`.jsx` thì `arrayOf` 3/3, `hunkAfter` 2/2, `gitBackendOf` 1/1, `titleRow` 1/1, `partitionOf` 2/2 (+4 test giấu đi), closure `register.redraw` 14/14, đều exact. Nghĩa là thuật toán tốt, chỉ hỏng ở khâu phân giải specifier.

### 6. yona/refs/oh-my-pi (bun workspace, ~6.100 file TS + crate Rust)

| # | Đích | rg | ct | Thời gian | Kết luận |
|---|---|---|---|---|---|
| O1 | `packages/utils/src/env.ts#isCompiledBinary` | 13 | **0** | lần đầu >120 s không ra gì; hỏi lại 78 ms | Kém |
| O2 | `env.ts#parseEnvFile` (trong cùng gói) | 10 (+5 test) | 10/0/0 exact | **20,3 s**, "a driver restarted", ~126 s CPU | Ngang (đúng nhưng chậm) |
| O3 | `env.ts#isWsl` | 9 | **0** | 97 ms | Kém |
| O4 | `abortable.ts#untilAborted` | 320 | **0** | 162 ms | Kém |
| O5 | `coding-agent/src/vibe/runtime.ts#VibeSessionRegistry.global` | 12 | 12/0/**23** (`flags.global`, `this.#global`, `target.global`…); `runtime.ts:1554` gắn unresolved dù viết thẳng `VibeSessionRegistry.global()` | **21,6 s**, driver restart | Kém |
| O6 | `VibeSessionRegistry.listIdsByOwner` (`X.global().listIdsByOwner()`) | 1 | 1/0/0 unresolved (kiểu trả về đã khai báo) | 114 ms | Ngang |

Phần oh-my-pi **chưa đủ yêu cầu**: chỉ có 6 đích (đề bài là 8–12), không có câu `locate` nào, không chạy truy vấn không giới hạn depth. Tôi dừng vì mỗi lần hỏi lại có thể kéo rust-analyzer lên 7 GB, RAM trống đã xuống còn 263 MB, trong khi một phiên khác đang chạy song song. Widget có 7 đích.

### Cỡ output từng đích (`--depth 1 --compact`, dòng/byte) so với số dòng rg

Số dòng rg = đúng + sai ở các bảng trên (rg `-n '\bten\b'`). Thời gian ấm của mọi đích là 1–5 ms (`Cost`), 17–30 ms thực; chỉ các câu lần đầu và oh-my-pi khác, đã ghi trong bảng thời gian và bảng O.

| Đích | ct | rg | Đích | ct | rg | Đích | ct | rg |
|---|---|---|---|---|---|---|---|---|
| D1 | 7/371 B | 13 | E1 | 11/708 B | 1 | J1 | 27/3,2 KB | ~64 |
| D2 | 20/2,4 KB | 27 | E2 | 13/1,0 KB | 7 | J2 | 28/4,0 KB | 9 |
| D3 | 19/2,3 KB | 31 | E3 | 10/538 B | 3 | J3 | 19/1,4 KB | 4 |
| D4 | 11/737 B | 3 | E4 | 10/629 B | 1 | J4 | 18/1,6 KB | ~50 |
| D6 | 11/824 B | 3 | E5 | 12/694 B | 6 | J5 | 12/810 B | 5 |
| D7 | 6/378 B | 4 | E6 | 11/715 B | 25 | J6 | 19/1,7 KB | ~10 |
| D8 | 11/870 B | 4 | E7 | 16/1,3 KB | 12 | J7 | 17/1,2 KB | 7 |
| D9 | 10/627 B | 5 | E8 | 11/740 B | 7 | M1 | 16/1,3 KB | 5 |
| D10 | 10/645 B | 3 | E9 | 14/1,1 KB | 7 | M2 | 15/1,1 KB | 5 |
| M3 | 11/749 B | 5 | E10 | 10/535 B | 2 | M4 | 11/731 B | 12 |
| M5 | 10/560 B | 20 | M6 | 10/587 B | 2 | M7 | 40/5,7 KB | ~131 |
| M8 | 44/6,5 KB | 6 | M9 | 123/19 KB | 120 | C7 | 78/14 KB | 165 |

## `locate` (3+ câu khó mỗi repo)

| Dòng | Kết quả | Đánh giá |
|---|---|---|
| debtnote `packages/rpc/src/index.ts:141` (arrow trong `.mutation(...)` trong object router lồng) | `appRouter` (:60) | Đúng nhưng thô |
| debtnote `tesseract-receipt-reader.ts:30` (method của object trả về) | `createTesseractReceiptReader.read` | Đúng |
| debtnote `owner-app.tsx:950` (JSX sâu) | `OwnerWorkspace` | Đúng |
| debtnote `receipt-reader.ts:10` (method của interface) | `(module)` | Chấp nhận được |
| extension `instagram.ts:81` (arrow làm giá trị khoá) | `instagramAdapter.isActorId` | Đúng |
| extension `url-state.ts:54` (khoá `set:` thứ ba trong mảng object) | `FIELDS.set` tại :40 | Thiếu: ba hàm `set` gộp làm một symbol |
| extension `background.ts:16` (callback trong `defineBackground`) | `(module)` | Thô |
| widget `Flow/FlowCheckout.vue:1162` (trong `getCart`, khai báo ở :1161) | `#default` | **Sai** (lỗi 4) |
| widget `FlowCheckout.vue:1170` (trong `nextScreen`) | `getCart` | **Sai** |
| widget `FlowCheckout.vue:1233,1236` (trong `handleOrder`) | `updateSubmission` | **Sai** |
| widget `stores/modules/toast.js:20` | `actions.error` | Đúng |
| metea `cms/pages/Products/Index.vue:376` | `columns.cell` (:361) | Đúng |
| metea `router/index.ts:190` (trong callback `beforeEach`) | `(module)` | Thô |
| metea `stores/content.ts:213` | `useContentStore.heroSlides` | Đúng |
| claude-code thật, mọi dòng | `Not found` | **Sai** (lỗi 3) |
| claude-code bản sao `register.ts:80`, `dialog-pane.tsx:76`, `partition-of.ts:25` | `register.backendHostOf.run`, `dialogPane`, `partitionOf.quiet` | Đúng |

## Nhận xét hai hook

**Hook trước khi tìm (rg/grep).** Trong bài test này nó gần như vô dụng, đôi khi còn sai:

- Nó tìm repo theo `cwd` của phiên (`src/hook.rs:75`: `tool_input.path`, nếu không có thì `input.cwd`). Lệnh Bash dạng `cd /repo && rg ...` hay `rg ten /repo/path` đều bị bỏ qua đường dẫn. Kết quả: hỏi nhầm repo `easy-ai-dev`. Bằng chứng: `rg -n '\.read\(' -g '!*.test.ts'` chạy trong debtnote nhận về "who calls `read`" là `scripts/gdrive.py#read` của easy-ai-dev. Sai hoàn toàn và gây nhiễu.
- Với tên không có trong repo của phiên thì không hiện gì (`formatVnd`, `calculateOutstandingAmount`, kể cả khi đưa đường dẫn tuyệt đối). Người dùng không biết hook đã bỏ qua.
- `searched_pattern` lấy từ đầu tiên không bắt đầu bằng `-` làm pattern, nên giá trị của cờ bị lấy nhầm. Với `rg -g '*.ts' foo` thì `*.ts` bị `searched_name` loại, hook chỉ im lặng. Nhưng `rg -t rust foo` sẽ hỏi về `rust` (đọc code thì thấy vậy, tôi chưa chạy thử).
- Phụ tác dụng: hook tự khởi động server cho `easy-ai-dev` (python + generic) và để nó chạy 30 phút.

**Hook sau khi sửa (Edit).** Có ích và nhanh. Sửa thân `arrayOf` trong bản sao thì hiện ngay 2 người gọi exact kèm dòng code. Hai điểm yếu:

- Sửa JSDoc ngay trên hàm thì bị coi là sửa `(module)`, nên chỉ hiện dòng `export * from './array-of'`, không hiện người gọi `arrayOf`.
- Trên repo thật dính lỗi 1–3, nó sẽ báo "0 người gọi" một cách tự tin. Đây là chỗ nguy hiểm nhất của hook: agent đọc thấy 0 sẽ yên tâm sửa.

## Lỗi theo mức nặng

### Nặng (bỏ sót im lặng, sai dữ liệu, cạn tài nguyên)

1. **Không phân giải gói workspace.** `drivers/javascript/driver.mjs:232-251` (`specifierCandidates`) chỉ xét đường dẫn tương đối, alias của Vite/tsconfig ở gốc và `baseUrl`. Specifier trần như `@debtnote/money-core` ra `module: null`, và lời gọi bị **bỏ hẳn**, không vào unresolved (README nói "never dropped"). Bằng chứng: D1 (`packages/application/src/ledger-service.ts:58` …), O1/O3/O4 (`untilAborted` 0/320). Symlink `apps/web/node_modules/@debtnote/money-core -> packages/money-core` và `exports.types = ./src/index.ts` có sẵn ở đó, chỉ là không được đọc.
2. **`.js`/`.jsx` không được ánh xạ sang `.ts`/`.tsx`.** `resolveSpecifier` (`driver.mjs:253-261`) chỉ thử `base`, `base+ext`, `base/index+ext`. Bằng chứng: claude-code `mods/diff/hooks/classify/index.ts:3` `export * from './is-noise-file.js'` → `isNoiseFile` 0 người gọi; sửa trong bản sao thì ra 2/2. Ở claude-code có 1.011 dòng `.js` và 21 dòng `.jsx`; ở oh-my-pi có 64 dòng.
3. **Repo không có `package.json` thì không có driver JS.** `drivers/javascript/driver.json` có `detect: [{file: package.json}]`. claude-code có tsconfig (`mods/tsconfig.json`) và 1.370 file `.ts` mà vẫn chạy generic: `path#name`, `path:line`, `locate` đều `Not found`, không có dòng nào nói vì sao.
4. **Số dòng Vue lệch.** Driver dùng `block.loc.start.line` (`driver.mjs:544`) và `prop.exp.loc.start.line` / `node.content.loc.start.line` (`:1272-1275`). Ở Vue 3.4.21 và cả 3.5.13 (bản đi kèm driver), giá trị `loc.start.line` này lớn hơn số dòng thật khi phía trên có thuộc tính nhiều dòng (`:class="\n…"`, `@error="\n…"`), còn `loc.start.offset` thì đúng. Kiểm bằng script: `CardProductOrder.vue` khối script `loc.line=56`, theo offset là 55; `toMoney(totalPrice)` 41 so với 40. 13/136 file `.vue` của widget lệch 1–8 dòng. Hậu quả: J1/J2/J3 in sai dòng và sai đoạn code, `locate` trả sai hàm (bảng `locate`), dấu vân tay `#hash` cũng lệch theo. metea không dính (0/232 file).
5. **oh-my-pi: câu hỏi TS bị Rust kéo theo.** Server khởi động `driver-lsp` → `rust-analyzer` 6–7 GB RSS. Lần hỏi đầu `isCompiledBinary` quá 120 s, không ra gì (00:28–00:30). O2 và O5 mất 20–21 s, ~126–133 s CPU, có "a driver restarted": rust-analyzer bị giết rồi khởi động lại, mỗi lần lại dựng chỉ mục. RAM trống có lúc còn 263 MB. `codetrace logs` chỉ có một dòng `server … started`, không ghi việc khởi động lại.
6. **Method của object trả về từ factory không có người gọi theo tên.** `createTesseractReceiptReader.read` ra 0, trong khi method class cùng tên `PostgresLedgerRepository.read` lại nhận 2 dòng `reader.read(...)` (biến có kiểu `ReceiptReader` khai báo rõ) làm unresolved. Bằng chứng: `apps/worker/src/receipt-readers/fallback-receipt-reader.ts:10`, `apps/worker/src/receipt-worker.ts:22`.

### Trung bình (nhiễu lớn, nhãn sai)

7. **Hằng cấp module coi mọi importer của file là "người gọi" exact.** `buttonVariants` (`components/ui/button/index.ts:6`) có 31 dòng `import { Button } from "@/components/ui/button"` gắn exact (M8). `date-picker-field.tsx#CalendarPicker` kéo 3 dòng `import { DatePickerField }`; `app.tsx#OwnerApp` kéo `main.tsx:4 import { App }`. Import một export khác của file không phải là dùng hằng này.
8. **Đọc thuộc tính trùng tên bị tính là "reference, unresolved".** Vòng 2 đã thấy với `.settings`/`.layout`, nay gặp lại: `actions.error` thừa 15 dòng `.error`; `VibeSessionRegistry.global` thừa 23 dòng `.global`; `FIELDS.set` khi không giới hạn depth kéo 10 dòng `.set(` (Map, storage). Tên càng phổ biến (`error`, `set`, `global`) càng thành rác.
9. **Lời gọi trong template qua template ref không được ghi lại**, kể cả dạng unresolved: `sliderRef?.scrollNext()` (`RCM/WidgetComboHorizontal.vue:123`). Gọi qua slot scope (`ChatView.vue:964` `scrollTo(index)`) cũng sót.
10. **Không đọc `.mdx`** dù `vite.config.js` của metea có plugin MDX: 19 dòng `<GuideCallout` sót, và cũng không có cảnh báo.
11. **Nhãn yếu:** `VibeSessionRegistry.global()` gọi thẳng theo tên class trong cùng file mà ra unresolved (`runtime.ts:1554`); `main: installZaloTab` ra `[handed, unresolved]`; `import(...).then(m => m.OwnerApp)` ra unresolved, và chuỗi dừng hẳn ở đó dù `--depth 3`.

### Nhẹ (output)

12. Mỗi câu trả lời ở widget đều kèm 2 dòng `Warning: … At least one <template> or <script> is required` (`ChatSidebar.vue`, `IconChatAskAI.vue`), không liên quan đến câu hỏi.
13. Dòng tự thân `return { update }` / `rowsOf,` (`[returned, unresolved]`) vẫn hiện trong Direct callers (J3, M1, M2).
14. Ở chế độ generic, phần "Unresolved, read by hand" lặp lại y nguyên Direct callers, nên output dài gấp đôi.
15. Khoá object trùng tên trong một mảng (`FIELDS.set` ×3, `columns.cell`) gộp làm một symbol mang dòng của khoá đầu tiên.
16. Lời gọi trong callback ẩn danh (`defineBackground(async () => …)`, `router.beforeEach(...)`) bị gán cho `(module)`.

## Đề xuất cải tiến

1. **Phân giải workspace (sửa lỗi 1).** Khi gặp specifier trần: (a) thử `node_modules/<tên>` gần nhất theo `realpath`. Symlink pnpm/bun trỏ thẳng vào `packages/x`; trong repo thì lấy file đó, còn ngoài repo thì coi là `package`. (b) Nếu không có `node_modules`, dựng bảng `name → thư mục` từ `workspaces` của `package.json`, hoặc `pnpm-workspace.yaml` (glob gói), rồi đọc `package.json` của từng gói và chọn `exports["."]` theo điều kiện `types` > `development` > `import` > `default`, sau đó `main`/`types`. Bảng này dựng một lần khi khởi động, chỉ dựng lại khi một `package.json` đổi. Với oh-my-pi, việc này mở khóa 18.117 import.
2. **Đổi `.js`/`.jsx`/`.mjs`/`.cjs` sang `.ts`/`.tsx`/`.mts`/`.cts`** trong `resolveSpecifier` khi `known` không có đúng file đó (sửa lỗi 2). Chỉ vài dòng, mà mở khóa claude-code.
3. **Bật driver JS theo `tsconfig.json`/`jsconfig.json`** hoặc theo số file `.ts`/`.js`, không chỉ theo `package.json` (sửa lỗi 3). Khi rơi về generic, in một dòng nói rõ lý do.
4. **Tính số dòng Vue từ `loc.start.offset`** qua `lineOfOffset` có sẵn (`driver.mjs:565`), không dùng `loc.start.line`; áp cho khối script (`:544`) và biểu thức template (`:1272-1275`, `:1300`) (sửa lỗi 4).
5. **Tách driver theo ngôn ngữ của câu hỏi.** Câu hỏi về file `.ts` không được chờ `rust-analyzer`. Khởi động rust-analyzer lười, chỉ khi có câu hỏi `.rs`. Đặt trần RAM, hoặc hỏi trước khi bật rust-analyzer với workspace lớn. Ghi vào log mỗi lần driver bị khởi động lại (sửa lỗi 5).
6. **Import không phân giải được thì vẫn ghi lời gọi**, dạng `unresolved` kèm tên specifier ("gọi `x` từ `@scope/pkg`, gói chưa đọc được"). Đúng như README hứa: thà thừa một dòng còn hơn sót. Thêm vào tổng kết một dòng "N import chưa phân giải" để người đọc biết con số 0 có đáng tin hay không.
7. **Hằng không phải hàm** (`const x = cva(...)`, `lazy(...)`): người gọi là chỗ tham chiếu đúng binding, không phải mọi importer của module (sửa lỗi 7). Cạnh "module nạp" chỉ nên dùng cho code cấp file có tác dụng phụ.
8. **Lọc phép đọc thuộc tính:** chỉ ghi `member` khi node là callee, hoặc khi được truyền đi như callback (đối số, giá trị thuộc tính, `v-bind`). Phép đọc trong điều kiện hay toán tử (`a.error ||`, `if (x.global)`) thì bỏ (sửa lỗi 8).
9. **Template Vue:** ghi `ref?.m()` / `ref.value?.m()` giống phía script. Nối `<slot v-bind="{ fn }">` với `v-slot="{ fn }"` của component cha.
10. **Kiểu khai báo:** biến hay tham số có chú thích kiểu interface (`reader: ReceiptReader`), hoặc static method có kiểu trả về khai báo (`global(): VibeSessionRegistry`), thì nâng lên `likely`, và nối với mọi object literal hay class được gán kiểu đó (sửa lỗi 6 và 11).
11. **Hook grep:** lấy repo từ đường dẫn trong lệnh (đối số sau pattern, `cd X &&`), bỏ qua giá trị của `-g/-t/-m/...` khi tìm pattern. Không đoán được repo thì im lặng, đừng hỏi repo của phiên.
12. **Hook edit:** sửa JSDoc/comment ngay trên khai báo thì quy về symbol bên dưới. Khi câu trả lời có import chưa phân giải, thêm cảnh báo "0 người gọi có thể chưa đủ".
13. **`.mdx`:** nếu `vite.config` có plugin MDX thì đọc thẻ JSX trong `.mdx`, ít nhất bằng regex `<Ten` nối với `MDXProvider :components`. Nếu không làm, thì cảnh báo rằng có N file `.mdx` chưa được đọc.
14. Gom `Warning:` về file không liên quan thành một dòng đếm, chỉ in chi tiết khi file đó nằm trong cây kết quả.

## Ghi chú môi trường

- `jam-easychatgpt-widget` là symlink tới `repos/easy-ai-platform-widget`; `codetrace stop` báo đúng đường dẫn thật. Server tôi dừng là server tôi tự mở.
- Một lệnh `rg` không có đường dẫn chạy trong tác vụ nền đã treo vì đọc stdin; đó là lỗi của tôi, không phải của codetrace. Đã dừng.
- Đã dừng mọi server tôi mở (debtnote, extension, widget, metea, claude-code, bản sao, oh-my-pi). Server `easy-ai-dev` do hook grep tự mở vẫn chạy (pid 2143067), tôi để nguyên vì có thể phiên khác đang dùng. Server `dash_yona` không phải của tôi.

```
Status: DONE_WITH_CONCERNS
Summary: 50 đích JS/TS/Vue/React trên 6 repo: hơn rg 16, ngang 21, kém 13. Trên repo một gói, codetrace chính xác và nhanh (1–5 ms); bỏ sót im lặng ở monorepo workspace, specifier .js→.ts, repo không có package.json; số dòng Vue lệch; oh-my-pi bị rust-analyzer làm treo hoặc chậm 20 s.
Concerns/Blockers: oh-my-pi chỉ 6 đích, không có locate, không có truy vấn không giới hạn depth vì rust-analyzer 7 GB (RAM trống còn 263 MB); widget 7 đích.
```
