# Test độc lập driver Python của codetrace (vòng 3)

Ngày 25/10/2026, 21:50–22:15. CLI `codetrace` (symlink tới `scripts/codetrace/codetrace`), không sửa gì trong `scripts/codetrace`.

Repo dùng:

- **hermes-agent** `/home/robertnguyen/www/yona/refs/hermes-agent` (chỉ đọc): 8.419 file `.py`, trong đó khoảng 2.500 file ngoài test. Repo có cả JS nên daemon chạy driver Python cùng driver generic.
- **fx3** (repo mẫu, tôi tự viết, `git init` trong scratchpad): `/tmp/claude-1000/-home-robertnguyen-www-easy-ai-dev/a6c5ca1b-1368-4a00-910f-bbe94707325a/scratchpad/py3/fx3`, 16 file, gom mọi dạng mới của vòng này. Tôi đã chạy thật `app.py` để lấy sự thật lúc chạy (ví dụ `D1().greet()` trả `'a1'`, `Diamond().greet()` trả `'right'`, `Grand().setup()` trả `['diamond','left','right','base']`).
- **hcopy** (bản sao để sửa): toàn bộ `.py` + `pyproject.toml`/`setup.py` của hermes chép vào `.../scratchpad/py3/hcopy`, `git init`, 8.420 file.

Cách đối chiếu: với mỗi đích, `rg` toàn repo theo tên, đọc tay từng dòng lệch so với `callers --depth 1 --compact`. Output thô nằm ở `.../scratchpad/py3/out/` và `.../scratchpad/py3/edits/`.

## Tóm tắt

- **Kiểm lại vòng 2:** cả 7 lỗi trên repo mẫu cũ (`fx`) và 2 lỗi trên backend đều đã hết. `helper` không còn nhận tham số lambda làm caller, `pkg.core.helper()` và import trong thân hàm được tìm thấy, override ở lớp trung gian không còn `exact` sai, template method ra `likely`, `class Sub(core.Base)` không còn bị tính là caller, thư mục script được ưu tiên đúng, `try/except ImportError` ra `likely`. Ở backend, `locate server.py:200` ra `#analyze`, `self.resize_image()` trong hàm lồng được tìm thấy.
- **Chạy đúng:** callback (`map`, `sorted(key=)`, `Thread(target=)`, `functools.partial`), `async`/`await`/`asyncio.gather`, generator và `yield from`, import vòng trong thân hàm, hàm trùng tên ở 2 module, re-export qua `__init__.py`, `@classmethod` gọi qua tên class từ file khác, chuỗi `super()` một nhánh. Sửa code rồi hỏi ngay thì kết quả luôn khớp daemon mới (20/20 câu so).
- **Lỗi nặng nhất:**
  1. Thêm hoặc xoá **một** file `.py` ở repo lớn thì câu hỏi kế tiếp mất 57–62 giây (đọc lại cả 8.420 file), kể cả khi đã chờ 90 giây sau khi xoá.
  2. Một câu hỏi lớn chặn mọi câu hỏi khác của cùng repo. Client đã bỏ cuộc mà daemon vẫn tính tiếp, `locate` bị treo hơn 1 phút.
  3. Hàm định nghĩa hai lần trong cùng scope (`@overload`, `@x.setter`, `if/else: def f`) làm `locate` và `callers file:dòng` trả sai symbol. Hermes có 226 chỗ như vậy, phủ 3.944 dòng. Có ca thật ra **0 caller** trong khi hàm có 10 caller.
- **`exact` sai:** MRO đa kế thừa đi sai thứ tự. Lời gọi `Cls.static_method()` bị tính là gọi `Cls.__init__`; ở hermes, 23/55 dòng caller của `BasePlatformAdapter.__init__` là kiểu này.
- **Bỏ sót im lặng:**
  - Mọi dunder được gọi ngầm (`__call__`, `__enter__/__exit__`, `__iter__`, `__getattr__`, `__len__`, `__post_init__`).
  - Constructor thừa kế (`SpecialCart()` không ra ở `Cart.__init__`).
  - Module được re-export bằng `from . import repo as storage`.
  - `getattr(obj, "tên")()`.
  - `super(Mixin, self)` trong mixin.
- **Nhiễu:** decorator dạng gọi (`@functools.wraps(fn)`, `@dataclass(frozen=True)`) thành "entry point". Hàm lồng tên `handler` kéo theo mọi chỗ đọc `x.handler` trong repo thành `unresolved`.
- **Tốc độ:** hỏi ấm phần lớn 1–25 ms (đạt). Riêng method của lớp cha có nhiều lớp con (`BasePlatformAdapter.*`) mất 700–1.250 ms **mỗi lần**, 5 lần liên tiếp không có cache. Lần đầu ở hermes mất 58 s và dùng 625 MB.

## Bảng đích (`callers --depth 1 --compact`)

Cost ở đây là số "Cost wall" của chính tool; client thêm khoảng 15–35 ms.

### Repo mẫu fx3

| # | Đích | Dạng khó | Cost | Kết quả | Sự thật (rg + đọc + chạy) | Đánh giá |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `shop/core.py#Cart.add` | gọi qua biến, `self`, `partial(Cart.add)`, `getattr(cart,"add")`, monkeypatch | 1 ms, 1,2 KB | 3 exact (`core.py:39,45` self, `dyn.py:24` partial), 7 unresolved | Thiếu `dyn.py:19–20` (`method = getattr(cart, "add")`; `method(5)`). `patch.py:9 Cart.add = fake_add` không được nhắc | Bỏ sót im lặng (getattr chuỗi hằng) |
| 2 | `#Cart.total` | `@property` + `@total.setter` | 1 ms | `core.py:67 self.total` exact, `app.py:18 cart.total` unresolved, `core.py:18 @total.setter` exact [reference] từ `Cart` | Đủ. Dòng `@total.setter` không phải lần dùng property | Nhiễu nhẹ |
| 3 | `#Cart.from_list` | `@classmethod` qua `cls` và tên class | 1 ms | `app.py:21` exact, `core.py:31 cls.from_list` exact, `core.py:75 Cart.from_list` **unresolved** | Đủ. Dòng 75 gọi qua tên class cùng file, chắc chắn là `Cart` | Nhãn thấp hơn mức có thể |
| 4 | `#Cart.empty` | `shop.Cart.empty()` qua package | 1 ms | 1 unresolved | Đủ | Chấp nhận |
| 5 | `#Cart.validate` | `@staticmethod` qua tên class và `self` | 1 ms | `app.py:20` exact, `core.py:40` exact, `core.py:38` (cùng file) unresolved | Đủ | Như #3 |
| 6 | `#Cart.__call__`, `__enter__`, `__exit__`, `__iter__`, `__getattr__`, `__len__` | gọi ngầm qua `cart(3)`, `with`, `for`, thuộc tính lạ, `len()` | 1 ms | **0 caller cả 6**, không cảnh báo | `app.py:17` (`cart(3)`), `:13` (`with Cart([1])`, cả enter lẫn exit), `:15` (`for`), `:19` (`cart.item_0`), `:24` (`len(cart)`), `core.py:71` (`len(self)`, chắc chắn) | **Bỏ sót im lặng** |
| 7 | `#Cart.__init__` | constructor | 1 ms | 3 caller, gồm `app.py:11,13,20,21` exact | `app.py:20 Cart.validate(2)` và `:21 Cart.from_list(...)` **không** tạo `Cart` → exact sai. Thiếu `core.py:24 cls()`. Sau lần sửa 4: thiếu `special.py:14 SpecialCart()` (lớp con không có `__init__`) | **Exact sai + bỏ sót im lặng** |
| 8 | `#Cart.checkout` | `@retry(times=3)` (decorator bọc hàm) | 1 ms | "Affected entry points: @retry(times=3)" | `retry` chỉ bọc, không đăng ký, nên không có lối vào nào | Entry point giả |
| 9 | `shop/deco.py#retry.decorate.wrapper`, `#timed.wrapper` | `@functools.wraps(func)` | 1 ms | "Affected entry points: @functools.wraps(func)" | `wraps` có trong `PLAIN_DECORATORS` nhưng dạng gọi không được nhận ra | Entry point giả |
| 10 | `shop/repo.py#Repo.save` | `self.repo = Repo()` trong `__init__`, rồi `self.repo.save()` | 1 ms | 4 unresolved (`repo.py:18,21`, `app.py:31`, `types_only.py:12`) | Đủ | Chấp nhận (có thể exact) |
| 11 | `#Repo`, `#Repo.__init__`, `#Service.__init__` | `from . import repo as storage` trong `shop/__init__.py`, rồi `storage.Repo()` | 1 ms | `Repo`: chỉ `repo.py:15`. `Service.__init__`: **0** | Thiếu `app.py:30 storage.Repo()` và `app.py:28 storage.Service()` | **Bỏ sót im lặng** |
| 12 | `shop/mro.py#A1.hello`, `#C1.hello` | `class D1(B1, C1)`, `B1(A1)`, `D1.greet: self.hello()` | 1 ms | `A1.hello`: **0**. `C1.hello`: `mro.py:51` **exact** | Chạy thật: `D1().greet() == 'a1'` (MRO D1, B1, A1, C1, Base) | **Exact sai + bỏ sót** |
| 13 | `#Right.setup`, `#Base.setup` | kim cương `Diamond(Left, Right)`, `super()` nhiều tầng | 1 ms | `Right.setup`: chỉ `app.py:39` unresolved. `Base.setup`: `Left.setup:11` và `Right.setup:16` exact | Chạy thật: `super().setup()` ở `Left.setup:11` với đối tượng `Diamond` gọi `Right.setup`. Thiếu `mro.py:11` ở `Right.setup`; ở `Base.setup` dòng 11 đáng ra là `likely` | Bỏ sót + exact quá tay |
| 14 | `#Diamond.setup`, `#Right.hello` | `super()` từ `Grand`, `self.hello()` qua MRO kim cương | 1 ms | `Grand.setup:32` exact; `Diamond.greet:27` exact | Đúng | Không |
| 15 | `shop/dyn.py#double`, `#by_price`, `#worker`, `core.py#make_cart` | `map(f)`, `sorted(key=f)`, `Thread(target=f)`, `partial(f)` + alias `new_cart` | 1 ms | Đủ, [reference]/[call] exact | Đúng | Không |
| 16 | `#Cart.safe_add` | `getattr(cart, "safe_add")(6)` | 1 ms | 1 unresolved (`types_only.py:20`) | Thiếu `dyn.py:21` | Bỏ sót im lặng |
| 17 | `shop/aio.py#fetch`, `#Client.get`, `#fetch_all` | `await`, `asyncio.gather(f(1), f(2))`, `create_task` trong comprehension, `gather(*(self.get(i) …))`, `asyncio.run` | 1 ms | Đủ, exact | Đúng | Không |
| 18 | `shop/gen.py#numbers`, `#evens` | genexp trong `yield from`, `yield from f()`, comprehension cấp module | 1 ms | Đủ, gồm `(module)` | Đúng | Không |
| 19 | `shop/circ_a.py#ping`, `circ_b.py#pong` | import vòng, `from .circ_a import ping` và `from . import circ_a` trong hàm | 1 ms | Đủ, exact | Đúng | Không |
| 20 | `shop/dupe_a.py#normalize`, `dupe_b.py#normalize` | trùng tên, `import shop.dupe_b as db` | 1 ms | Mỗi bên đúng phần của mình | Đúng | Không |
| 21 | `shop/dc.py#Money.__post_init__` | dataclass | 1 ms | **0** | `Money(5)` `app.py:44`, `Money(1)` `:46`, `Money(...)` `dc.py:14` | **Bỏ sót im lặng** |
| 22 | `shop/dc.py#Money` | `@dataclass(frozen=True)` | 1 ms | 3 caller đúng + "entry point @dataclass(frozen=True)" | Không phải lối vào | Entry point giả |
| 23 | `shop/types_only.py` (`if TYPE_CHECKING:` + `def fill(cart: Cart)`) | type hint | 1 ms | `cart.add(1)` unresolved | Type hint không được dùng để resolve. Dòng chú thích `types_only.py:10` cũng không hiện ở `#Cart` (trong khi `price: Money` trong thân class thì có) | Chấp nhận, thiếu nhất quán |
| 24 | `shop/patch.py#fake_add` | `Cart.add = fake_add` | 1 ms | `patch.py:9` exact [reference] | Đúng. Nhưng `Cart.add` không biết là đã bị thay | Giới hạn chấp nhận được |

### hermes-agent (repo thật)

| # | Đích | Dạng khó | Cost | Kết quả | Sự thật | Đánh giá |
| --- | --- | --- | --- | --- | --- | --- |
| H1 | `hermes_time.py#now` | `from hermes_time import now as _hermes_now`, `as _now`, import trong hàm, `import hermes_time; hermes_time.now()` | 58.017 ms (lần đầu), sau đó 18–25 ms; 8,3 KB | 64 exact, 33 trong test | Khớp danh sách import của `rg`; `hermes_cli/cron.py:122` (import trong hàm `:117`) có mặt | Không |
| H2 | `hermes_state_wal.py#_log_once` | `functools.partial(_log_once, …)` cấp module | 13 ms | 6 caller, gồm `(module) :575,576` | Đúng. Nhưng người gọi thật qua biến partial (`:288,342,379,440`) không với tới: chuỗi dừng ở `(module)` | Giới hạn (nên ghi chú) |
| H3 | `tools/code_execution_rpc.py#_rpc_poll_loop` | `Thread(target=wrap(f))`, re-export qua `code_execution_tool`, import trong hàm (`code_kernel_remote.py:370`) | 14 ms | 2 exact | Đúng 2/2 | Không |
| H4 | `tools/terminal_tool_backends.py#_build_sandbox_env` | 3 `partial` | 14 ms | `(module) :204,205,206` | Đúng (dispatch qua dict `_ENV_BUILDERS` thì không lần được) | Không |
| H5 | `mcp_serve.py#EventBridge._poll_loop` | `Thread(target=self._poll_loop)` | 18 ms | `:292` exact [reference] | Đúng | Không |
| H6 | `tools/process_registry.py#ProcessSession.__post_init__` | dataclass thật | 20 ms | **0** | `#ProcessSession` có 5 chỗ tạo (`process_registry.py:1112`, `process_registry_results.py:110`, `process_registry_checkpoint.py:110`, `goal_scope_probe.py:25` ×2) | **Bỏ sót im lặng** |
| H7 | `tools/yuanbao_tools.py#_yb_tool`, `#_yb_tool.deco.handler` | decorator có tham số + `@functools.wraps(fn)` | 14–15 ms | `_yb_tool`: 5 dòng `@_yb_tool(...)` đúng. `handler`: entry point giả `@functools.wraps(fn)` + **12 unresolved** là các chỗ đọc `.handler` ở `registry.py`, `console_engine.py`, `plugins.py`… | Hàm lồng không thể gọi qua thuộc tính, nên 12 dòng đó là nhiễu | Nhiễu |
| H8 | `#get_group_info` | hàm bị `@_yb_tool("…")` bọc, đăng ký qua list `:297` | 14 ms | entry `@_yb_tool('get_group_info')` + `:297` | Hợp lý | Không |
| H9 | `tools/delegate_tool_progress.py#_ChildProgressRelay.__call__`, `tools/mcp_tool.py#__getattr__` (PEP 562) | gọi ngầm | 13–18 ms | 0 / 0 | Đúng là chỉ gọi ngầm, nhưng không có dòng nào báo đây là method gọi ngầm | Im lặng (như #6) |
| H10 | `gateway/hosted_room_peer.py#GatewayRoomCatalog.from_mapping` | `@classmethod`, cùng tên ở 5 class khác | 16 ms | 5 exact + 4 unresolved | 5 exact đúng (`hosted_room_links.py:73`, `api_server_room_dispatch.py:85`, `hosted_room_peer_http.py:588`, `hosted_room_service.py:594`, `methods_groups.py:304,314`). Trong 4 unresolved: `hosted_room_peer.py:273` là chính nó (nên exact); 3 dòng còn lại là `RoomExecutionPolicy.from_mapping`, `LoopCapConfig.from_mapping`, `StoredRoomLink.from_mapping`, chắc chắn là class khác | Nhiễu + nhãn thấp |
| H11 | `tools/tts_streaming.py#SentenceChunker.from_config` | classmethod, mọi chỗ import trong thân hàm | 18 ms | 4 exact | Đúng 4/4 (`audio.py:681` tôi suýt bỏ sót khi `rg`) | Không |
| H12 | `gateway/platforms/base.py#BasePlatformAdapter.send_image` | `super()` từ khoảng 30 adapter, mixin | **681–1.253 ms mỗi lần** | 10 exact (9 `super()` + `self.send_image`), 1 unresolved | Thiếu `plugins/platforms/discord/adapter_media.py:421` `super(DiscordMediaMixin, self).send_image(...)` (mixin không có lớp cha; `DiscordAdapter(DiscordMediaMixin, …)`). Các `self.send_image` trong lớp con có override bị loại đúng | Bỏ sót + chậm |
| H13 | `#BasePlatformAdapter.__init__` | constructor | 1.243 ms | 51 caller exact | **23/55 dòng `[call]`** là `BasePlatformAdapter.filter_media_delivery_paths(...)`, `._is_partial_delivery(...)`… (static/classmethod), không tạo đối tượng | **Exact sai hàng loạt** |
| H14 | `gateway/platforms/qqbot/onboard.py#qr_register` | re-export ở `qqbot/__init__.py` + `__all__` + import trong hàm; `feishu/adapter.py#qr_register` trùng tên | 15 ms | `gateway_setup_wizard.py:531` exact | Đúng, bản feishu bị loại đúng | Không |
| H15 | `agent/delegation_context.py#delegated_child_subprocess_env` (theo tên) | 2 `@overload` + bản thật | 16 ms | 10 caller | Đủ caller, nhưng symbol trỏ dòng 166 (stub `@overload` đầu), không phải bản thật dòng 171 | Xem lỗi locate |

## Hỏi không giới hạn `--depth` (hermes, `timeout 120`)

| Đích | Thời gian | Output | Symbol duy nhất / dòng cây / sâu nhất | Entry point | Đếm |
| --- | --- | --- | --- | --- | --- |
| `hermes_time.py#now` | 4,3 s | 580 KB, 3.150 dòng | 1.314 / 3.063 / ~75 cấp | 81 (12 `__main__`, các `@_session_method(...)`, `@_pet_method(...)`…) | 566 exact, 10 likely, 2.125 unresolved |
| `BasePlatformAdapter.send_image` | 19,7 s | 258 KB | 295 / 1.218 / ~63 | 6, trong đó 3 dòng là decorator mô tả của cùng một lệnh Discord (`@discord.app_commands.describe`, `@…autocomplete`, `@tree.command`) | 231 / 1 / 870 |
| `hermes_constants.py#get_hermes_home` | **Hết 120 s, không có output** (rc 124). Chạy lại với `timeout 590`: **212,6 s** | **46,8 MB**, 234.434 dòng cây | 26.632 symbol | 744 | 14.288 / 324 / **202.919 unresolved**; bộ nhớ 1.482 MB (daemon giữ 1.440 MB sau đó) |

Treo khi client bỏ cuộc (lần chạy `get_hermes_home` đầu):

- 22:02:25: chạy `timeout 120 codetrace --repo …/hermes-agent callers 'hermes_constants.py#get_hermes_home'`. 22:04:25: client bị giết.
- 22:04:25: `ps` cho thấy `codetrace serve` STAT `Sl` WCHAN `anon_pipe_write`, `python3 driver.py` STAT `R` 37% CPU (đang tính tiếp câu đã bị bỏ). Log `/tmp/codetrace-robertnguyen/a1180e364d243d8a.log` chỉ có `server 1670557 started for …/hermes-agent`.
- 22:04:32: `timeout 30 codetrace locate hermes_time.py:150` (câu dễ nhất) **hết 30 s** (rc 143). Lúc này serve ở `futex_do_wait`.
- 22:05:53: daemon mới trả lời lại.

Lần chạy lại mất 212 s nên lần đầu không để lại cache gì. Một agent hỏi rộng là repo đó đứng 3–4 phút với mọi câu hỏi khác, và cơ chế "kẹt 20 s thì khởi động lại" không kích hoạt vì driver vẫn đang bận thật.

## locate

| Vị trí | Nội dung dòng | Kết quả | Đúng? |
| --- | --- | --- | --- |
| fx3 `shop/patch.py:11` | `Cart.checkout = lambda self: original(self)` | `#install` | Đúng |
| fx3 `shop/gen.py:15` | comprehension cấp module | `#(module)` | Đúng |
| fx3 `shop/gen.py:7` | genexp trong `yield from` | `#evens` | Đúng |
| fx3 `shop/core.py:65` | `@retry(times=3)` | `#Cart` | Chấp nhận được (Python cũng chạy decorator trong thân class), dù người đọc hay mong `Cart.checkout` |
| fx3 `shop/core.py:13` | dòng trống giữa method | `#Cart` | Đúng |
| fx3 `shop/core.py:20` | thân `@total.setter` | `#Cart` | **Sai**: phải là setter `Cart.total` |
| fx3 `shop/deco.py:11` | trong `wrapper` lồng 3 tầng | `#retry.decorate.wrapper` | Đúng |
| fx3 `shop/deco.py:15`, `:16` | dòng cuối `decorate` / `retry` | `#retry.decorate`, `#retry` | Đúng |
| fx3 `app.py:50` | trong `if __name__ == "__main__"` | `#(module)` | Đúng |
| fx3 `shop/dc.py:7` | trường dataclass | `#Money` | Đúng |
| fx3 `shop/aio.py:21` | genexp trong `gather` | `#Client.get_many` | Đúng |
| fx3 `shop/nested.py:2` | docstring module 2 dòng | `#(module)` | Đúng |
| fx3 `shop/nested.py:6` | lambda trong dict cấp module | `#(module)` | Đúng |
| fx3 `shop/nested.py:11` | docstring class | `#Outer` | Đúng |
| fx3 `shop/nested.py:15`, `:16` | comprehension 2 dòng trong method của class lồng | `#Outer.Inner.method` | Đúng |
| fx3 `shop/nested.py:20`, `:22` | hàm lồng / dòng sau hàm lồng | `#Outer.run.helper`, `#Outer.run` | Đúng |
| fx3 `shop/nested.py:27` / `:30` | thân `platform_name` nhánh `if` / nhánh `else` | `#platform_name` / **`#(module)`** | Nhánh `else` **sai** |
| fx3 `shop/nested.py:37` | comment sau `return` cuối | `#(module)` | Chấp nhận (comment không bao giờ lên traceback) |
| hermes `hermes_cli/backup.py:195` | thân `_lock_op` thứ nhất (Windows) | `#_backup_operation_lock._lock_op` | Đúng |
| hermes `hermes_cli/backup.py:201` | thân `_lock_op` thứ hai (`fcntl.flock`) | `#_backup_operation_lock` | **Sai** |
| hermes `agent/delegation_context.py:178`, `:182` | thân hàm thật sau 2 `@overload` | **`#(module)`**; `callers …:182` ra **0 caller** | **Sai, hậu quả nặng** (hàm có 10 caller) |
| hermes `agent/context_compressor.py:2207` | thân `@context_length.setter` | `#ContextCompressor` | **Sai** |
| hermes `tools/yuanbao_tools.py:109` | `@_yb_tool("get_group_info")` | `#(module)` | Chấp nhận (như `core.py:65`) |
| hermes `tools/yuanbao_tools.py:46` | `await fn(args)` trong `handler` lồng | `#_yb_tool.deco.handler` | Đúng |
| hermes `hermes_state_wal.py:575` | `functools.partial` cấp module | `#(module)` | Đúng |

Đếm bằng `ast` trên hermes (ngoài test): 226 định nghĩa trùng key trong cùng scope (215 `if/else`/`try` và hàm lồng, 8 setter/deleter, 3 overload), phủ **3.944 dòng**. Mọi dòng này `locate` ra symbol cha, và `callers <file:dòng>` hỏi nhầm symbol.

## Sửa code trong lúc hỏi

"Ngay" là hỏi liền sau khi lưu file. "Mới" là sau khi `pkill -f "[r]elease/codetrace serve <bản sao>$"`. Driver con cũng tắt theo, không thấy tiến trình mồ côi.

| # | Repo | Sửa | Hỏi ngay (Cost) | Kết quả ngay | So với daemon mới |
| --- | --- | --- | --- | --- | --- |
| E1 | fx3 | Thêm `def extra(): return normalize("x"), use_text()` vào `app.py` | 1 ms | `app.py:56` xuất hiện ở `dupe_a.normalize` và `use_text` | Giống hệt |
| E2 | fx3 | Chèn 3 dòng đầu `shop/core.py` | 1 ms | `Cart.add` dời từ 10 lên 13; caller `:42,48`; `locate :13` ra `#Cart.add`; dấu vân tay `#be56` giữ nguyên | Giống hệt (3/3) |
| E3 | fx3 | Đổi tên `fetch` → `fetch_one` | 1 ms | Tên mới đủ caller, tên cũ "Not found" rc=1 | Giống hệt |
| E4 | fx3 | File mới `shop/special.py`: `SpecialCart(Cart)` override `add`, gọi `super().add`, `self.add`, `self.safe_add` | 11 ms | `Cart.add` thêm `special.py:6` (super) exact; `SpecialCart.add` có `special.py:9` exact và `core.py:42,48` likely; `Cart.safe_add` thêm `special.py:10` exact | Giống hệt (4/4). Thiếu `SpecialCart()` `special.py:14` ở `Cart.__init__` ở cả hai |
| E5 | fx3 | `from shop.dupe_a import normalize` → `from shop.dupe_b import normalize` | 0–1 ms | `dupe_a.normalize`: 0; `dupe_b.normalize`: `app.py:35,56`, `dupe_b.py:6` | Giống hệt |
| E6 | fx3 | `git rm shop/gen.py shop/circ_b.py` | 1–14 ms | `gen.py#numbers` "Not found"; `circ_a.ping` hết caller | Nội dung giống; daemon ấm in thêm dòng `Direct callers (0):`, daemon mới không in |
| E6b | fx3 | Hỏi `callers numbers` (tên hàm đã xoá) và `locate shop/gen.py:2` | 1 ms | `numbers … Total: 0` rc=0; `locate` in lại `shop/gen.py:2` rc=0 | Giống. Không báo "Not found" |
| H-E1 | hcopy | Thêm hàm gọi `SentenceChunker.from_config` vào `utils.py` | 21 ms | `utils.py:790` xuất hiện | (kiểm chung ở H-E3) |
| H-E2 | hcopy | Chèn 5 dòng đầu `gateway/platforms/base.py` (file 3.000+ dòng) | `locate` 9 ms; `callers send_image` 1.110 ms | Dời đúng (`:3044`, caller `:3029,3056`) | Giống hệt |
| H-E3 | hcopy | Tạo file mới `tools/new_probe_mod.py` | **61.857 ms** | `new_probe_mod.py:5` xuất hiện, đúng | Giống hệt |
| H-E4 | hcopy | `git rm tools/code_kernel_remote.py`, **chờ 90 s**, rồi hỏi | **57.190 ms** | `code_kernel_remote.py:383` biến mất, đúng | Giống hệt; daemon mới cold mất 54.390 ms |

## Lỗi theo mức nặng

### Nặng

1. **Thêm hoặc xoá một file `.py` làm daemon đọc lại cả repo, và chỉ đọc khi có người hỏi.**
   - Bằng chứng: H-E3 61,9 s, H-E4 57,2 s (đã chờ 90 s sau khi xoá, nên không có việc làm sẵn ở nền).
   - Nguyên nhân: `Store.sync` (`drivers/python/driver.py:463–472`) gặp `reshaped` thì dựng lại `Modules` và `facts.clear()`. Câu `callers` kế tiếp gọi `read_all` (`:576`) parse lại 8.420 file.
   - Repo lớn mà mỗi lần tạo module mới, đổi nhánh git hay sinh file test là một lần khởi động lạnh. Điều này trái với README ("lúc bạn hỏi thì phần lớn việc đã xong").
2. **Một câu hỏi lớn chặn cả repo, kể cả sau khi client đã bỏ.** Xem mục "Treo" ở trên: `locate` hết 30 s, 1 phút 28 giây sau khi client chết daemon mới trả lời lại. Câu không giới hạn `get_hermes_home` mất 212 s, ra 46,8 MB và 202.919 dòng unresolved, nên một agent gọi qua MCP mà quên `--depth` sẽ làm treo mọi agent khác trên repo đó.
3. **Định nghĩa trùng trong cùng scope làm `locate`/`callers file:dòng` trả sai, có ca ra 0 caller.**
   - Ca thật: `agent/delegation_context.py:182` (thân hàm thật sau 2 `@overload`) cho ra `#(module)` và "Direct callers (0)", trong khi `delegated_child_subprocess_env` có 10 caller.
   - Thêm `hermes_cli/backup.py:201`, `agent/context_compressor.py:2207`, fx3 `shop/core.py:20`, `shop/nested.py:30`.
   - Phạm vi ở hermes: 226 chỗ, 3.944 dòng.
   - Nguyên nhân: `declare` dùng `declarations.setdefault(key, [start, end])` (`driver.py:191`), nên chỉ giữ khoảng dòng của định nghĩa đầu. Symbol theo tên cũng trỏ vào stub `@overload` (dòng 166) thay vì bản thật.
4. **`Cls.static_or_classmethod()` bị tính là gọi `Cls.__init__`, gắn `exact`.**
   - Bằng chứng: fx3 `app.py:20,21`; hermes 23/55 dòng `[call]` của `BasePlatformAdapter.__init__` (ví dụ `cron/scheduler_delivery.py:1137`, `gateway/delivery.py:331`).
   - Nguyên nhân: `aliases` thêm `(path, key, None)` và `item is None` khớp mọi `member` (`driver.py:493, 628`), nên `Cart.validate(...)` có `kind=call` thành caller của `#Cart`. `:616–619` lại lấy mọi `call` của lớp làm caller của `__init__`.
5. **MRO đa kế thừa sai thứ tự, ra `exact` sai và bỏ sót.**
   - Bằng chứng: fx3 `mro.py:51`, `D1(B1, C1)`, `B1(A1)`. Chạy thật trả `'a1'`, nhưng tool gán `D1.greet` là caller exact của `C1.hello`, còn `A1.hello` 0 caller.
   - Nguyên nhân: `ancestors_of` (`driver.py:527–537`) duyệt bằng `pending.pop()` (lấy base cuối trước, không phải C3), rồi `first_definer` (`:539`) lấy phần tử đầu theo thứ tự đó.

### Vừa

6. **Dunder gọi ngầm luôn 0 caller, không cảnh báo:** `__call__`, `__enter__`, `__exit__`, `__iter__`, `__getattr__`, `__len__`, `__post_init__` (fx3 #6, #21; hermes H6, H9). Ít nhất `len(self)` (`core.py:71`), `with Cart(...)` (`app.py:13`) và `Money(...)` cho `__post_init__` là chắc chắn. Nếu chưa làm thì nên in một dòng kiểu "method được Python gọi ngầm; caller dưới đây chưa đầy đủ".
7. **Constructor thừa kế:** `SpecialCart()` (`special.py:14`, lớp con không có `__init__`) và `cls()` trong classmethod (`core.py:24`) không ra ở `Cart.__init__`.
8. **Module re-export bằng `from . import x as y`:** `storage.Repo()` (`app.py:30`) và `storage.Service()` (`app.py:28`) bị sót. Nguyên nhân: `bind_import` chỉ ghi `reexports` khi `binding[2] != "*"` (`driver.py:169`).
9. **`super(Mixin, self)` trong mixin không có lớp cha:** `discord/adapter_media.py:421` bị sót ở `BasePlatformAdapter.send_image`. `ancestors_of(DiscordMediaMixin)` rỗng, nên tham chiếu `super` bị bỏ thay vì gắn `unresolved`.
10. **`getattr(obj, "hằng")(...)`** (`dyn.py:19–21`) bị sót, dù README hứa tên động "luôn được liệt kê unresolved".
11. **Method của lớp cha có nhiều lớp con chậm mỗi lần hỏi, không cache.** `BasePlatformAdapter.send_image` 681–1.253 ms (5 lần liên tiếp), `.send` 993 ms, `.__init__` 1.243 ms. `subclasses_of` (`driver.py:556–570`) quét mọi file cho mỗi lớp trong hàng đợi; `ancestors_of` chạy lại cho mỗi tham chiếu `super`.
12. **Decorator dạng gọi thành entry point giả:** `@functools.wraps(func)`, `@dataclass(frozen=True)`, `@retry(times=3)`. `PLAIN_DECORATORS` chỉ khớp được `Name`/`Attribute`, còn decorator dạng `ast.Call` cho ra `name=None` (`driver.py:197`). `@retry` là decorator tự viết chỉ bọc hàm; khó phân biệt với decorator đăng ký, nhưng ít nhất `wraps`/`dataclass`/`lru_cache(...)` nên được loại.
13. **Hàm lồng bị đối xử như method:** `_yb_tool.deco.handler` kéo 12 dòng `x.handler` khắp repo thành unresolved (H7). Nhánh `to == "member" and "." in key` (`driver.py:637`) không kiểm `owner` có phải class không.

### Nhẹ

14. **Tên trần gõ sai hoặc đã xoá thì ra "0 caller", rc=0:** `callers get_hermes_hom`, `callers numbers`, `callers doesnotexist_xyz`. Trong khi dạng `file#key` báo `Not found` rc=1. Agent có thể hiểu nhầm là "không ai gọi, xoá được". `locate` file không tồn tại thì in lại chính đầu vào, rc=0.
15. **Gọi qua tên class cùng file luôn `unresolved`** (`core.py:38,75`, `hosted_room_peer.py:273`), và `X.from_mapping` của class khác cùng file cũng bị liệt kê (H10).
16. **Biến gán từ constructor không được lần theo** (`cart = Cart(); cart.add()`, `Diamond().greet()`), type hint `def f(x: Cart)` không được dùng. Có ghi `unresolved` nên không sót, nhưng README nói dạng `const cart = useCart(); cart.add()` được lần theo, và với Python thì chưa đúng.
17. **Partial cấp module chặn chuỗi caller:** `_log_wal_fallback_once = partial(_log_once, …)` dừng ở `(module)`, các chỗ gọi `_log_wal_fallback_once()` không lên cây (H2).
18. **Lặt vặt:**
    - Daemon ấm và daemon mới khác nhau ở dòng `Direct callers (0):` (E6).
    - `@x.setter` được ghi là tham chiếu exact tới property.
    - Bộ nhớ daemon hermes lên 1,44 GB sau câu hỏi lớn và không nhả ra.
    - Entry point Discord bị đếm 3 lần cho một lệnh (`describe`, `autocomplete`, `command`).

## Phần chạy đúng

- **Callback:** callback và hàm truyền làm giá trị (`map`, `sorted(key=)`, `Thread(target=f)`, `Thread(target=self.m)`, `Thread(target=wrap(f))`, `functools.partial`) đều ra `[reference]` exact, cả ở fx3 lẫn hermes (H3, H4, H5).
- **Async và generator:** `async def`/`await`/`asyncio.gather`/`create_task`/`asyncio.run`, generator, `yield from`, genexp, comprehension cấp module.
- **Import:** import vòng với import trong thân hàm (`from .x import f`, `from . import x`), hàm trùng tên ở hai module, `import a.b as c`. Alias `from m import f as g` hoạt động ở quy mô lớn: 64/64 caller của `now`. Re-export qua `__init__.py` + `__all__` (H14).
- **Lớp và method:**
  - `@classmethod` gọi qua `cls.` và qua tên class import từ file khác (`SentenceChunker.from_config` 4/4, `GatewayRoomCatalog.from_mapping` 5/5).
  - `@property` qua `self.` ra exact.
  - `super()` một nhánh và nhiều tầng tuyến tính (`Grand → Diamond → Left`); `super()` từ khoảng 30 adapter lên `BasePlatformAdapter.send_image`.
  - Override ở lớp con: `self.add` trong `Cart` ra `likely` cho `SpecialCart.add`; `self.safe_add` thừa kế ra exact.
- **`locate`:** đúng với lambda, comprehension nhiều dòng, class lồng, hàm lồng 3 tầng, docstring, dòng cuối hàm, code cấp module. Riêng các dòng thuộc định nghĩa trùng thì sai, xem lỗi 3.
- **Cập nhật khi sửa file:** sửa nội dung, chèn dòng, đổi tên, đổi import, thêm/xoá file đều đúng ngay ở lần hỏi kế tiếp và khớp daemon mới ở cả 20 câu so. Chỉ có lỗi về thời gian khi thêm/xoá file.
- **Tốc độ khi ấm:** fx3 0–1 ms; hermes phần lớn 13–25 ms (trừ lỗi 11). Dấu vân tay dòng ổn định khi dòng chỉ dời chỗ.

## Ghi chú

- Không sửa gì trong `scripts/codetrace` (`git status` vẫn chỉ `?? scripts/codetrace/` như lúc đầu; `drivers/python/` không có `__pycache__`), không sửa hermes-agent (`git status` sạch). Không commit.
- Mọi sửa đổi chỉ làm trên `.../scratchpad/py3/fx3` và `.../scratchpad/py3/hcopy`.
- Cuối buổi tôi đã tắt đúng 3 daemon của mình (fx3, hcopy, hermes-agent), không đụng daemon của phiên Rust/PHP và backend.
