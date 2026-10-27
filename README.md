<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/lockup-dark.svg">
    <img src="docs/brand/lockup-light.svg" alt="scout" width="300">
  </picture>
</h1>

<p align="center">
  <a href="https://github.com/locdev-2001/scout/stargazers"><img src="https://img.shields.io/github/stars/locdev-2001/scout?style=flat&logo=github" alt="GitHub stars"></a>
  <img src="https://img.shields.io/badge/Built%20with-Rust-000000?logo=rust&logoColor=white" alt="Built with Rust">
  <img src="https://img.shields.io/badge/MCP-Claude%20Code%20%C2%B7%20Codex-C8F03C" alt="MCP: Claude Code, Codex">
  <img src="https://img.shields.io/badge/PHP%20%C2%B7%20Laravel%20%C2%B7%20JS%2FTS%20%C2%B7%20Vue%20%C2%B7%20Python%20%C2%B7%20Rust-555555" alt="PHP, Laravel, JS/TS, Vue, Python, Rust">
</p>

<p align="center">
  <strong>Trước khi sửa một hàm: ai đang gọi nó? Đang cầm một dòng lỗi: dòng này nằm trong hàm nào, và hàm đó được gọi từ đâu?</strong><br>
  Trả lời cho cả người lẫn AI agent, dựa trên code đang nằm trên máy bạn ngay lúc hỏi.
</p>

---

scout là một lệnh dòng lệnh kèm MCP server. Bạn đưa tên một hàm (hoặc `file:dòng` trong stack trace), scout trả về cây người gọi, đi ngược lên tới nơi người dùng chạm vào: một route, một lệnh, một sự kiện. Mỗi người gọi có độ chắc chắn và vị trí chính xác.

## Vì sao

`grep` tên hàm cho ra mọi chỗ trùng tên, kể cả chỗ không liên quan, và bỏ sót chỗ gọi qua interface, facade hay quy ước của framework. Agent sửa code dựa trên kết quả đó thì hay sót. scout làm khác:

- Mỗi ngôn ngữ dùng chính bộ phân tích của ngôn ngữ đó (tokenizer PHP, parser TypeScript, `ast` của CPython, `rust-analyzer`), không đoán bằng regex.
- Thà thừa còn hơn sót: chỗ nào không chắc vẫn được liệt kê, gắn `unresolved`, để bạn đọc tay.
- Kết quả luôn khớp code hiện tại: file vừa lưu được cập nhật ngay, và mỗi lần hỏi đều kiểm lại để không trả lời bằng dữ liệu cũ.
- Agent dùng được ngay: MCP server cho Claude Code và Codex, cùng hook để agent thấy người gọi ngay khi sửa hay grep một hàm, và được nhắc trước khi kết thúc lượt nếu còn hàm đã đổi mà chưa xem người gọi.

scout không thay grep. Nó trả lời một câu: đổi chỗ này thì ảnh hưởng tới đâu. Tìm định nghĩa, chuỗi, khoá cấu hình, dòng log vẫn là việc của `grep`/`rg`.

## Ví dụ

Giả sử dự án có class `OrderService` với hàm `place()`:

```bash
scout callers 'OrderService::place'
```

```text
OrderService::place  app/Services/OrderService.php:42#a1f3
├─ CheckoutController::store  [call]  app/Http/Controllers/CheckoutController.php:30#9c2e
│  └─ route POST /checkout  [route]
└─ RetryFailedOrders::handle  [call]  app/Jobs/RetryFailedOrders.php:18#47d0
   └─ RetryFailedOrders::__construct  [queue]
```

## Cách hoạt động

```
scout callers X ──► tiến trình nền của dự án ──► driver theo ngôn ngữ ──► cây người gọi
   (CLI / MCP)        (theo dõi file vừa lưu,      (PHP · Laravel · JS/TS ·      + lối vào bị ảnh hưởng
                       cache câu trả lời)           Python · rust-analyzer)
```

- Lần hỏi đầu, scout khởi động một tiến trình nền cho dự án. Nó tự tắt sau 10 phút không ai hỏi để trả RAM; phiên sau Python/JS/PHP khởi động lại nhanh nhờ cache, Rust lớn 15–40 giây.
- Tiến trình nền để ý file nào bạn vừa lưu và cập nhật ngay, nên lúc bạn hỏi thì phần lớn việc đã xong. Mỗi lần hỏi nó vẫn kiểm lại nhanh toàn bộ file.
- Hỏi lại lần hai gần như tức thì: câu trả lời được giữ tới khi file liên quan đổi.
- Python, JavaScript/TypeScript và PHP lưu kết quả đọc file vào `~/.cache/scout` (vài MB đến vài chục MB mỗi dự án). Lần hỏi đầu của phiên sau từ 11–15 giây còn khoảng 1 giây trên dự án 6.000–8.000 file; backend Laravel 5.000 file từ khoảng 90 giây còn khoảng 10 giây. File đổi trong lúc tiến trình nền tắt vẫn được đọc lại. `scout status` báo dung lượng, `scout uninstall` xoá nó.
- Git worktree mới của một dự án PHP đã có cache: scout lấy kết quả đọc của worktree anh em mới nhất làm điểm xuất phát, rồi chỉ đọc lại những file khác. Backend Laravel 5.000 file: CPU warm-up từ khoảng 29 giây còn 1–5 giây, kết quả giống hệt khi đọc từ đầu.
- Nhiều dự án cùng khởi động không giành hết máy. Việc đọc cả dự án lúc khởi động chờ một trong các "slot" chung của máy (mặc định nửa số nhân), và không chạy khi RAM trống dưới 1,5 GB. Khi tổng RAM của mọi tiến trình nền vượt ngân sách (mặc định 40 % RAM máy), tiến trình lâu không được hỏi nhất bị tắt. Tiến trình đang trả lời hay vừa được hỏi trong 1 phút thì không bao giờ bị tắt.

Mỗi ngôn ngữ hiểu code bằng công cụ của chính nó:

| Ngôn ngữ | Bộ phân tích |
|---|---|
| PHP | tokenizer và Reflection của PHP |
| Laravel | khởi động app thật để đọc route, sự kiện, lệnh |
| JavaScript, TypeScript, React, Vue, Svelte, MDX | parser TypeScript, trình dịch Vue/Svelte, cấu hình Vite của dự án (alias, component tự import, file vào) |
| Python | `ast` của CPython, không cần cài thêm |
| Rust | `rust-analyzer` (`rustup component add rust-analyzer`) |

## Tính năng

- [x] `scout callers` theo tên hàm, theo `file#tên`, hoặc theo `file:dòng` lấy từ stack trace
- [x] `scout locate file:dòng`: dòng này thuộc hàm nào
- [x] Cây người gọi đi tới lối vào: route, lệnh artisan, sự kiện, lịch chạy, code nằm thẳng trong file
- [x] Ba mức chắc chắn: `exact`, `likely`, `unresolved`
- [x] `--compact` cho hàm dùng khắp nơi; `--tests` để liệt kê cả chỗ gọi trong test
- [x] Laravel: scope, accessor, middleware, form request, policy, facade, `__call`, macro
- [x] Monorepo pnpm, yarn, bun, npm workspace: import gói nội bộ được nối tới file nguồn qua `exports`/`types`/`main`
- [x] Lời gọi ngầm của Python (`with`, `for`, `len(x)`, `__getattr__`...) và Rust (`?`, `Display`, `Iterator`, `Deref`...)
- [x] `scout locate file:dòng,dòng,…`: các hàm quanh nhiều dòng một lần
- [x] MCP server (`callers`, `locate`) và hook cho Claude Code: người gọi của hàm vừa đổi dù đổi bằng cách nào, nhắc một lần cuối lượt
- [x] Ngôn ngữ khác (C/C++, Java, C#, Go...): mọi chỗ gọi theo tên, gắn `unresolved`

## Cài đặt

Cần Rust (`cargo`, cài ở https://rustup.rs). Rồi:

```bash
git clone https://github.com/locdev-2001/scout.git ~/www/scout && ~/www/scout/install.sh
```

Lệnh này build công cụ rồi chạy `scout setup`, làm trọn mọi bước:

- tạo lệnh `scout` trong `~/.local/bin`;
- gắn MCP server cho Claude Code và Codex (agent khác: thêm MCP server chạy `scout mcp`);
- gắn hook cho Claude Code:
  - phiên vừa mở: tiến trình nền của repo bắt đầu đọc code, và scout ghi lại commit gốc để biết phiên này đổi gì;
  - sau mỗi lần agent đổi code, dù bằng `Edit`, `Write` hay một lệnh shell (`sed -i`, script Python): scout so file với commit gốc, tìm hàm quanh các dòng đổi và đưa ngay danh sách người gọi của hàm agent chưa xem. Lệnh shell không đổi file nào chỉ tốn một lần `git status`;
  - trước khi agent kết thúc lượt (gồm cả thay đổi do subagent làm): nếu còn hàm đã đổi trong phiên mà agent chưa xem người gọi, scout đưa danh sách đó và agent đi tiếp một vòng để kiểm. Mỗi hàm chỉ được nhắc một lần, kể cả khi agent đã commit giữa chừng;
  - agent grep một tên hàm (kể cả trong `$( … )` hay nhiều lần tìm nối nhau): nó nhận thêm danh sách người gọi chính xác. Lệnh grep vẫn chạy như thường, hook chỉ thêm 15 ms ở trung vị và không bao giờ quá 1,5 giây; grep lọc output của lệnh khác hay tìm trong `.env`, `.md`, `.json` thì bị bỏ qua. Lần grep đầu tiên trong một repo chưa có tiến trình nền sẽ bật nó lên, nên có thể chưa kèm kết quả;
  - agent tự hỏi `callers` (MCP hay CLI) thì hàm đó được tính là đã xem;
- thêm vài dòng hướng dẫn vào `~/.claude/CLAUDE.md` và `~/.codex/AGENTS.md`;
- cài bộ đọc JavaScript dự phòng cho dự án không có `typescript` riêng;
- báo máy có những gì để phân tích code (PHP, Node, Python, rust-analyzer).

> [!TIP]
> Muốn xem trước mà không thay đổi gì: `install.sh --dry-run`.

### Quản lý

```bash
scout status          # đã gắn vào đâu, tiến trình nền nào đang chạy
scout stop [repo]     # dừng tiến trình nền (lần hỏi sau tự chạy lại)
scout logs [repo]     # cuối log của tiến trình nền
scout update          # kéo code mới của công cụ, build lại, cài lại
scout uninstall       # gỡ mọi thứ setup đã thêm
```

`scout stop` dừng cả các bộ phân tích tiến trình nền đã mở, kể cả `rust-analyzer` đang nạp dở.

`scout status` còn báo RAM của từng tiến trình nền, số slot đang dùng, và theo từng ngày: hook đã thêm được danh sách người gọi vào bao nhiêu phần trăm lần agent tìm kiếm hay đổi code, và ở bao nhiêu phần trăm lần kết thúc lượt agent đã xem người gọi của mọi hàm nó đổi (`finish … all-shown`), kèm lý do những lần không đạt. Có thể chỉnh bằng biến môi trường:

| Biến | Mặc định | Ý nghĩa |
|---|---|---|
| `SCOUT_MAX_WORKERS` | nửa số nhân | số việc đọc dự án chạy cùng lúc trên cả máy; `0` = không giới hạn |
| `SCOUT_MIN_AVAILABLE_MB` | `1536` | RAM trống tối thiểu để bắt đầu một việc đọc |
| `SCOUT_MEMORY_BUDGET` | 40 % RAM máy (MB) | trần RAM của mọi tiến trình nền cộng lại; `0` = không giới hạn |

`scout setup` còn cài skill `scout` cho Claude Code (`~/.claude/skills/scout/SKILL.md`) và đăng ký MCP với `alwaysLoad`, để tool `callers` được nạp cùng phiên thay vì nằm trong danh sách phải tìm mới dùng được.

## Cách hỏi

PHP dùng tên class:

```bash
scout callers 'OrderService::place'
```

Đang cầm một dòng trong stack trace? Hỏi thẳng bằng file và số dòng:

```bash
scout callers app/Services/OrderService.php:57
scout locate  app/Services/OrderService.php:57    # chỉ hỏi dòng này thuộc hàm nào
```

JavaScript, TypeScript, Vue, Svelte, React, Python và Rust viết symbol dạng `file#tên`:

```bash
scout callers 'src/utils/price.js#formatPrice'
scout callers 'src/components/CartItem.vue#default'   # ai đang dùng component này
scout callers 'app/billing.py#Invoice.total'           # method của class
scout callers 'src/cart.rs#Cart::add'                  # Rust dùng `::` như trong code
scout callers formatPrice                              # hoặc chỉ tên, nếu không trùng
```

- `--tests`: liệt kê cả chỗ gọi nằm trong test (mặc định chỉ đếm).
- `--compact`: hàm dùng khắp nơi thì cây có thể dài hàng nghìn dòng. Bản gọn cho người gọi trực tiếp kèm dòng code, rồi mọi chỗ khác bị ảnh hưởng gom theo file, rồi các dòng cần đọc tay. Agent dùng qua MCP nhận bản gọn này mặc định.
- `--depth`: giới hạn độ sâu của cây.

## Đọc kết quả

Mỗi dòng cho biết **ai gọi**, **chắc chắn tới đâu**, và **ở chỗ nào**:

| Mức | Nghĩa |
|---|---|
| `exact` | chắc chắn, vì kiểu dữ liệu được khai báo rõ |
| `likely` | gần như chắc, ví dụ gọi qua class cha hoặc interface |
| `unresolved` | tool không đoán được biến đó là gì, nên vẫn liệt kê để bạn tự nhìn qua |

- "Affected entry points" là những lối vào bị ảnh hưởng: route, lệnh artisan, sự kiện, lịch chạy. Code nằm thẳng trong file (không trong hàm nào, như `routes/console.php`) chạy khi file được nạp, nên cũng được tính là lối vào.
- Đuôi `#a1f3` là dấu vân tay của dòng code đó. Sửa dòng ấy thì dấu vân tay đổi, nên dễ biết kết quả cũ còn khớp hay không.
- Hỏi một tên không tồn tại (gõ sai, đã xoá) thì ra `Not found`.

## Giới hạn

scout nói thẳng chỗ nó không chắc thay vì im lặng bỏ qua.

### Mọi ngôn ngữ

- Code gọi hàm bằng tên lấy từ biến, kiểu `$obj->$tenHam()` hay `$tenClass::taoMoi()`, thì không công cụ nào đoán chắc được. Những chỗ này luôn được liệt kê `unresolved`.
- Gọi qua một đối tượng không rõ từ đâu ra (`item.save()` khi `item` được truyền vào từ chỗ khác) thì tool chỉ biết theo tên, nên liệt kê `unresolved`. Với PHP, dòng như vậy chỉ được liệt kê ở file có nhắc tên class (nếu không, mọi `->save()` trong dự án sẽ hiện ra). Riêng kết quả của một hàm trong dự án (`const { save } = useCart()`, `const cart = useCart(); cart.add()`) thì tool lần theo được.
- Hook theo dõi thay đổi chỉ thấy hàm còn trong code: hàm bị xoá hẳn hay đổi tên thì chỉ hàm bao quanh chỗ xoá được hỏi, người gọi của tên cũ phải hỏi tay. File mới (chưa có ở commit gốc của phiên) không được hỏi, vì trước phiên chưa ai gọi nó. Lệnh shell sửa file ở một repo khác thư mục làm việc chỉ được thấy khi phiên đã từng sửa repo đó bằng `Edit`/`Write`. Agent đụng vào một file đã sửa dở từ trước phiên thì cả thay đổi cũ trong file đó cũng được hỏi.
- Import trỏ vào chính dự án mà không tìm ra file (đường dẫn sai, gói chưa build) thì các lời gọi qua nó vẫn được liệt kê `unresolved`, không bị bỏ.
- Một câu hỏi rất rộng (không giới hạn `--depth` trên hàm dùng khắp nơi) dừng sau 10 giây, trả phần đã đi được và nói rõ chỗ bị cắt; hỏi tiếp về một người gọi trong đó để đi sâu hơn. Nếu bạn ngắt lệnh, tiến trình nền dừng câu đó ngay, không bắt câu sau phải chờ.
- Nếu một bộ phân tích ngôn ngữ bị kẹt (không làm gì suốt 20 giây), scout khởi động lại nó và hỏi lại một lần. Vẫn kẹt thì câu trả lời có thêm cảnh báo ngôn ngữ đó chưa trả lời được, thay vì treo mãi.

### PHP và Laravel

- Hàm được cất vào thuộc tính để gọi sau (`$this->xuLy = [$this, 'guiThu']`) là `unresolved`: ai gọi nó là chỗ đọc thuộc tính đó.
- Method trong class vô danh (`new class extends Mau { ... }`) có tên dạng `ClassBaoNgoai@anonymous_<dòng>::tenHam`. Người gọi của nó là người gọi cùng method ở class cha hoặc interface, gắn `likely`.
- Nếu một file làm PHP dừng hẳn khi nạp, scout báo rõ file đó chưa phân tích được. File thuộc phần test chỉ được báo khi bạn hỏi kèm `--tests`.
- Laravel gọi nhiều thứ theo quy ước, không viết tên ra. Tool nối được: scope `scopeDangBan` ← `->dangBan()`, accessor `getHoTenAttribute` ← `->ho_ten`, middleware ← các route nó gắn (gom thành một dòng khi nhiều), form request ← action nhận nó, policy ← `authorize('sua', ...)`/`can(...)`, class phía sau facade ← `Cache::remember(...)`, `__call` ← lời gọi method không khai báo, macro (`Builder::macro('tenMacro', ...)` thành method `Builder::tenMacro`) ← mọi `->tenMacro(`.
- Dự án lớn: lần đầu tiến trình nền đọc hết mọi file (khoảng 30 giây với 5.000 file). Câu hỏi đến trong lúc đó vẫn đúng nhưng mất vài giây; sau đó mỗi câu vài chục mili giây.
- PHP dùng `rg` (ripgrep) để tìm file nhanh nếu máy có; không có thì scout dựng một lần chỉ mục từ → file (khoảng 0,7 giây với 5.000 file) và chỉ đọc những file có chữ cần tìm, cùng kết quả.
- Laravel: scout khởi động app thật bằng `.env` của dự án, nên app nào truy vấn database lúc khởi động thì scout cũng truy vấn database đó. Khởi động lỗi (ví dụ không vào được database) thì route, sự kiện, lệnh không được nối và câu trả lời có cảnh báo; scout thử lại khi có file đổi hoặc sau 30 giây.
- Trên Windows, PHP không có `pcntl`: scout vẫn đúng nhưng sau mỗi lần sửa file PHP sẽ chậm (driver PHP khởi động lại).

### JavaScript, TypeScript và MDX

- Monorepo: `import { x } from '@cty/utils'` được nối tới file nguồn của gói đó qua `exports`/`types`/`main`; `./util.js` kiểu ESM của TypeScript được hiểu là `./util.ts`.
- MDX: chỉ đọc phần `import`/`export` và các thẻ component (`<Callout>`); biểu thức `{...}` giữa văn bản thì không.

### Python

- Lời gọi ngầm `with X()`, `for x in X()`, `x()`, `len(x)`/`str(x)`/..., `x.thuoc_tinh_khong_co` được nối tới `__enter__`/`__exit__`, `__iter__`, `__call__`, `__len__`/`__str__`/..., `__getattr__`.
- Tiến trình nền đọc trước toàn bộ dự án bằng vài tiến trình con (khoảng 12 giây với 8.400 file).

### Rust

- `rust-analyzer` chỉ được bật khi có câu hỏi về file `.rs` (dự án chủ yếu TypeScript có kèm một crate không phải trả giá cho nó). Lần hỏi Rust đầu chờ nó dựng chỉ mục (15–45 giây với workspace 5.000 file), và nó giữ vài GB RAM (khoảng 4–6,5 GB với workspace đó).
- Trong workspace nhiều crate, lời gọi trùng tên nằm ở crate không phụ thuộc crate đích được loại ngay từ `Cargo.toml`. Lần hỏi đầu cho một hàm dùng rộng có thể mất vài giây; hỏi lại thì nhanh.
- Hỏi ngay trong tích tắc vừa lưu một file lớn mất khoảng 0,5–1 giây (rust-analyzer phân tích lại); hỏi sau đó một giây thì vài chục mili giây.
- Lời gọi ngầm được liệt kê `unresolved` trong hàm có nhắc kiểu đích: `?`/`.into()` cho `From`, `{}`/`.to_string()` cho `Display`, `{:?}` cho `Debug`, `for ... in` cho `Iterator`/`IntoIterator`, `*x` cho `Deref`.
- Hàm do `macro_rules!` sinh ra có tên dạng `file.rs#ten@dong` (dòng viết `fn ten` trong macro).
- scout chỉ đọc code, không chạy build script hay proc-macro của dự án, và không tự tải toolchain mà `rust-toolchain.toml` ghim (máy thiếu toolchain đó thì câu trả lời kèm cảnh báo). Code chỉ biên dịch trên hệ điều hành khác (`#[cfg(windows)]`) được liệt kê `unresolved`.

## Công nghệ

Rust (CLI, tiến trình nền, MCP) · driver PHP, Node.js, Python · [`rust-analyzer`](https://rust-analyzer.github.io) · [`notify`](https://github.com/notify-rs/notify) theo dõi file · [`ignore`](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore) duyệt cây thư mục

## Đóng góp

Mỗi ngôn ngữ là một driver trong `drivers/<ngôn ngữ>/` (khai báo ở `driver.json`), phần Rust trong `src/` lo tiến trình nền, cache và cây người gọi. Mở issue hoặc PR vào nhánh `main`.

## ⭐ Star History

[![Star History Chart](https://api.star-history.com/svg?repos=locdev-2001/scout&type=Date)](https://star-history.com/#locdev-2001/scout&Date)
