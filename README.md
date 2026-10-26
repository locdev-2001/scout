# scout

Trước khi sửa một hàm, bạn thường muốn biết: **ai đang gọi nó?** Và nếu đang cầm một dòng lỗi trong log: **dòng này nằm trong hàm nào, và hàm đó được gọi từ đâu?**

scout trả lời đúng hai câu đó, cho cả người lẫn AI agent, dựa trên code đang nằm trên máy bạn ngay lúc hỏi.

## Cài một lần

Cần Rust (`cargo`, cài ở https://rustup.rs). Rồi:

```bash
git clone <repo> ~/www/scout && ~/www/scout/install.sh
```

Lệnh này build công cụ rồi chạy `scout setup`, làm trọn mọi bước:

- tạo lệnh `scout` trong `~/.local/bin`;
- gắn MCP server cho Claude Code và Codex (agent khác: thêm MCP server chạy `scout mcp`);
- gắn hai hook cho Claude Code: sửa một hàm xong, agent thấy ngay những chỗ gọi hàm đó; agent grep một tên hàm, nó nhận thêm danh sách người gọi chính xác (lệnh grep vẫn chạy như thường);
- thêm vài dòng hướng dẫn vào `~/.claude/CLAUDE.md` và `~/.codex/AGENTS.md`;
- cài bộ đọc JavaScript dự phòng cho dự án không có `typescript` riêng;
- báo máy có những gì để phân tích code (PHP, Node, Python, rust-analyzer).

Muốn xem trước mà không thay đổi gì: `install.sh --dry-run`.

## Quản lý

```bash
scout status          # đã gắn vào đâu, tiến trình nền nào đang chạy
scout stop [repo]     # dừng tiến trình nền (lần hỏi sau tự chạy lại)
scout logs [repo]     # cuối log của tiến trình nền
scout update          # kéo code mới của công cụ, build lại, cài lại
scout uninstall       # gỡ mọi thứ setup đã thêm
```

## Hỏi thế nào

Giả sử dự án có một class `OrderService` với hàm `place()`:

```bash
scout callers 'OrderService::place'
```

Kết quả là một cây, đi ngược từ hàm đó lên tới nơi người dùng chạm vào (một route, một lệnh, một sự kiện):

```text
OrderService::place  app/Services/OrderService.php:42#a1f3
├─ CheckoutController::store  [call]  app/Http/Controllers/CheckoutController.php:30#9c2e
│  └─ route POST /checkout  [route]
└─ RetryFailedOrders::handle  [call]  app/Jobs/RetryFailedOrders.php:18#47d0
   └─ RetryFailedOrders::__construct  [queue]
```

Đang cầm một dòng trong stack trace? Hỏi thẳng bằng file và số dòng:

```bash
scout callers app/Services/OrderService.php:57
scout locate  app/Services/OrderService.php:57    # chỉ hỏi dòng này thuộc hàm nào
```

Thêm `--tests` nếu muốn thấy cả những chỗ gọi nằm trong test (mặc định chỉ đếm, không liệt kê).

Hàm dùng ở khắp nơi thì cây có thể dài hàng nghìn dòng. Thêm `--compact` để nhận bản gọn: người gọi trực tiếp kèm dòng code, rồi mọi chỗ khác bị ảnh hưởng gom theo file, rồi các dòng cần đọc tay. Agent dùng qua MCP nhận bản gọn này mặc định.

Với JavaScript, TypeScript, Vue, Svelte, React, Python và Rust, symbol viết dạng `file#tên`:

```bash
scout callers 'src/utils/price.js#formatPrice'
scout callers 'src/components/CartItem.vue#default'   # ai đang dùng component này
scout callers 'app/billing.py#Invoice.total'           # method của class
scout callers 'src/cart.rs#Cart::add'                  # Rust dùng `::` như trong code
scout callers formatPrice                              # hoặc chỉ tên, nếu không trùng
```

## Đọc kết quả

Mỗi dòng cho biết **ai gọi**, **chắc chắn tới đâu**, và **ở chỗ nào**:

- `exact`: chắc chắn, vì kiểu dữ liệu được khai báo rõ.
- `likely`: gần như chắc, ví dụ gọi qua class cha hoặc interface.
- `unresolved`: tool không đoán được biến đó là gì, nên vẫn liệt kê ra để bạn tự nhìn qua. Thà thừa một dòng còn hơn sót.

Phần "Affected entry points" là những lối vào bị ảnh hưởng: route, lệnh artisan, sự kiện, lịch chạy. Code nằm thẳng trong file (không trong hàm nào, như `routes/console.php`) chạy khi file được nạp, nên cũng được tính là lối vào.

Đuôi `#a1f3` là dấu vân tay của dòng code đó. Nếu bạn sửa dòng ấy, dấu vân tay đổi, nên dễ biết kết quả cũ còn khớp hay không.

## Vì sao nhanh mà vẫn đúng

- Lần hỏi đầu, scout khởi động một tiến trình chạy nền cho dự án. Tiến trình này tự tắt sau 30 phút không ai hỏi.
- Nó để ý file nào bạn vừa lưu và cập nhật ngay, nên lúc bạn hỏi thì phần lớn việc đã xong.
- Mỗi lần hỏi, nó vẫn kiểm lại nhanh toàn bộ file để chắc không trả lời bằng dữ liệu cũ.
- Mỗi ngôn ngữ dùng chính "bộ não" của ngôn ngữ đó để hiểu code:
  - PHP: tokenizer và Reflection của PHP; Laravel: khởi động app thật để đọc route, sự kiện, lệnh.
  - JavaScript, TypeScript, React, Vue, Svelte, MDX: parser TypeScript, trình dịch Vue/Svelte và cấu hình Vite của chính dự án (alias, component tự import, file vào). Monorepo (pnpm, yarn, bun, npm workspace): `import { x } from '@cty/utils'` được nối tới file nguồn của gói đó qua `exports`/`types`/`main`; `./util.js` kiểu ESM của TypeScript được hiểu là `./util.ts`.
  - Python: `ast` của chính CPython, không cần cài gì thêm.
  - Rust: `rust-analyzer` (cài một lần bằng `rustup component add rust-analyzer`).
- Hỏi lại lần hai thì gần như tức thì: câu trả lời được giữ lại tới khi file liên quan đổi.
- Python và JavaScript/TypeScript lưu kết quả đọc file vào `~/.cache/scout` (vài MB đến vài chục MB mỗi dự án), nên phiên làm việc sau không phải đọc lại cả dự án: lần hỏi đầu từ 11–15 giây còn khoảng 1 giây trên dự án 6.000–8.000 file. File nào đổi (kể cả lúc tiến trình nền đang tắt) được đọc lại. `scout status` báo dung lượng, `scout uninstall` xoá nó.

## Những gì nó không làm được (và nói thẳng ra)

- Code gọi hàm bằng tên lấy từ biến, kiểu `$obj->$tenHam()` hay `$tenClass::taoMoi()`, thì không công cụ nào đoán chắc được. Những chỗ này luôn được liệt kê `unresolved` để bạn đọc tay.
- Hàm được cất vào thuộc tính để gọi sau (`$this->xuLy = [$this, 'guiThu']`) cũng là `unresolved`: ai gọi nó là chỗ đọc thuộc tính đó.
- Method trong class vô danh (`new class extends Mau { ... }`) có tên dạng `ClassBaoNgoai@anonymous_<dòng>::tenHam`. Người gọi của nó là người gọi cùng method ở class cha hoặc interface, gắn `likely`.
- Nếu một file làm PHP dừng hẳn khi nạp, scout sẽ báo rõ file đó chưa phân tích được. File thuộc phần test chỉ được báo khi bạn hỏi kèm `--tests`.
- Nếu một bộ phân tích ngôn ngữ bị kẹt (không trả lời, không làm gì suốt 20 giây), scout khởi động lại nó và hỏi lại một lần. Nếu vẫn kẹt, câu trả lời có thêm cảnh báo ngôn ngữ đó chưa trả lời được, thay vì treo mãi.
- Hỗ trợ sâu: PHP, Laravel, JavaScript, TypeScript, React, Vue, Svelte, Python, Rust. Ngôn ngữ khác (C/C++, Java, C#, Go...) vẫn được liệt kê mọi chỗ gọi theo tên, gắn `unresolved`.
- Gọi qua một đối tượng không rõ từ đâu ra (`item.save()` khi `item` được truyền vào từ chỗ khác) thì tool chỉ biết theo tên, nên liệt kê `unresolved`. Với PHP, dòng như vậy chỉ được liệt kê ở file có nhắc tên class (nếu không, mọi `->save()` trong dự án sẽ hiện ra). Riêng kết quả của một hàm trong dự án (`const { save } = useCart()`, `const cart = useCart(); cart.add()`) thì tool lần theo được.
- Rust: hỏi ngay trong tích tắc vừa lưu một file lớn mất khoảng 0,5–1 giây (rust-analyzer phân tích lại); hỏi sau đó một giây thì vài chục mili giây.
- Rust: `rust-analyzer` chỉ được bật khi có câu hỏi về file `.rs` (dự án chủ yếu TypeScript có kèm một crate không phải trả giá cho nó). Lần hỏi Rust đầu chờ nó dựng chỉ mục (15–45 giây với workspace 5.000 file), và nó giữ vài GB RAM (khoảng 4–6,5 GB với workspace đó). Trong workspace nhiều crate, lời gọi trùng tên nằm ở crate không phụ thuộc crate đích được loại ngay từ `Cargo.toml`. Lần hỏi đầu cho một hàm dùng rộng có thể mất vài giây; hỏi lại thì nhanh.
- Rust: scout chỉ đọc code, không chạy build script hay proc-macro của dự án, và không tự tải toolchain mà `rust-toolchain.toml` ghim (máy thiếu toolchain đó thì câu trả lời kèm cảnh báo). Code chỉ biên dịch trên hệ điều hành khác (`#[cfg(windows)]`) được liệt kê `unresolved`.
- `scout stop` dừng cả các bộ phân tích tiến trình nền đã mở (kể cả `rust-analyzer` đang nạp dở).
- Laravel gọi nhiều thứ theo quy ước, không viết tên ra. Tool nối được: scope `scopeDangBan` ← `->dangBan()`, accessor `getHoTenAttribute` ← `->ho_ten`, middleware ← các route nó gắn (gom thành một dòng khi nhiều), form request ← action nhận nó, policy ← `authorize('sua', ...)`/`can(...)`, class phía sau facade ← `Cache::remember(...)`, `__call` ← lời gọi method không khai báo, macro (`Builder::macro('tenMacro', ...)` thành method `Builder::tenMacro`) ← mọi `->tenMacro(`.
- Lời gọi ngầm: Python `with X()`, `for x in X()`, `x()`, `len(x)`/`str(x)`/..., `x.thuoc_tinh_khong_co` được nối tới `__enter__`/`__exit__`, `__iter__`, `__call__`, `__len__`/`__str__`/..., `__getattr__`. Rust liệt kê `unresolved` trong hàm có nhắc kiểu đích: `?`/`.into()` cho `From`, `{}`/`.to_string()` cho `Display`, `{:?}` cho `Debug`, `for ... in` cho `Iterator`/`IntoIterator`, `*x` cho `Deref`.
- Hàm do `macro_rules!` của Rust sinh ra có tên dạng `file.rs#ten@dong` (dòng viết `fn ten` trong macro).
- Hỏi một tên không tồn tại (gõ sai, đã xoá) thì ra `Not found`.
- Import trỏ vào chính dự án mà không tìm ra file (đường dẫn sai, gói chưa build) thì các lời gọi qua nó vẫn được liệt kê `unresolved`, không bị bỏ.
- MDX: chỉ đọc phần `import`/`export` và các thẻ component (`<Callout>`); biểu thức `{...}` giữa văn bản thì không.
- PHP dự án lớn: lần đầu tiến trình nền đọc hết mọi file (khoảng 30 giây với 5.000 file). Câu hỏi đến trong lúc đó vẫn đúng nhưng mất vài giây; sau đó mỗi câu vài chục mili giây. Python cũng đọc trước như vậy, bằng vài tiến trình con (khoảng 12 giây với 8.400 file).
- Một câu hỏi rất rộng (không giới hạn `--depth` trên hàm dùng khắp nơi) dừng sau 10 giây, trả phần đã đi được và nói rõ chỗ bị cắt; hỏi tiếp về một người gọi trong đó để đi sâu hơn. Nếu bạn ngắt lệnh, tiến trình nền dừng câu đó ngay, không bắt câu sau phải chờ.
- PHP dùng `rg` (ripgrep) để tìm file nhanh nếu máy có; không có thì scout tự đọc file, cùng kết quả nhưng câu đầu chậm hơn (backend Laravel 5.000 file: khoảng 9 giây thay vì 3).
- Trên Windows, PHP không có `pcntl`: scout vẫn đúng nhưng sau mỗi lần sửa file PHP sẽ chậm (driver PHP khởi động lại).
