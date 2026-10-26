# Test độc lập driver Python của codetrace (đợt C, vòng 5)

Ngày 26/10/2026, 00:20–01:13. CLI `codetrace` → `/home/robertnguyen/www/codetrace/codetrace`. Không sửa gì trong `/home/robertnguyen/www/codetrace`, hermes-agent, oh-my-pi, codex (`git status` sạch như lúc đầu).

Repo:

- **hermes-agent** (8.419 `.py`), **oh-my-pi** (186 `.py`), **codex** (163 `.py`): chỉ đọc.
- **fx5** `.../scratchpad/py5/fx5`: bản sao fx3 vòng 3, thêm `shop/hard.py` (dunder toán tử, metaclass, `__init_subclass__`, descriptor, `@overload`, `singledispatch`, walrus, `match`, closure) và `shop/sub/deep.py` (`from .. import`). Đã chạy thật để lấy sự thật lúc chạy.
- **hcopy** `.../scratchpad/py3/hcopy` (bản sao hermes vòng 3) để thêm/xoá file.

Output thô: `.../scratchpad/py5/out/`.

## Tóm tắt

- **Hơn rg:** hầu hết đích có import rõ ràng.
  - Ví dụ: `super()` qua ~30 adapter, `super(Mixin, self)`, template method `self._command` (60 dòng `likely`).
  - Script import thư mục kề (`import squad`, `from runtime import …`): 43/43, 5/5.
  - `with RpcClient(...) as c: c.prompt_and_wait()`: `exact`.
  - Override được loại đúng.
- **Ngang rg:** codex 5/5 đích. `get_hermes_home` depth 1: 586 exact, 5 unresolved đúng (`bind_module` gắn tên động).
- **Kém rg (lỗi mới, nặng):**
  1. Gói khai báo `package-dir = {"robomp" = "src"}` (oh-my-pi): mọi `from robomp.x import y` bị bỏ **im lặng**. `issue_key` ra **0 caller**, rg ra 14 (ngoài test) + ~60 (test).
  2. Đối tượng lấy từ **hàm** import (`broker = get_broker(); broker.m()`) hoặc **instance cấp module** import (`from tools.registry import registry; registry.get_entry()`): lời gọi method bị bỏ im lặng, không cả `unresolved`. Ví dụ:
     - `ToolRegistry.get_entry`: ct 3 dòng, rg 11 dòng thật của `ToolRegistry` (thiếu 9).
     - `ProcessRegistry.list_sessions`: thiếu 6/8.
     - `BrowserControlBroker.scope_for_session`: thiếu 2/3.
  3. Dunder ngầm giờ có caller chắc chắn, nhưng **ngập nhiễu**:
     - `SessionDB.__enter__`: 4 exact đúng chìm trong 2.072 dòng `unresolved` (249 KB ở depth 1).
     - `_ChildProgressRelay.__call__`: 3.492 unresolved (357 KB).
     - rg `with SessionDB(` ra đúng 4 dòng.
- **Vòng 3 đã sửa:** 10/13 lỗi hết hẳn, 3 lỗi còn một phần. Hai lỗi nặng nhất về thời gian đã hết: thêm/xoá file 57–62 s → 0,19 s, `BasePlatformAdapter.*` 0,7–1,25 s → 13–43 ms. Còn: câu hỏi lớn vẫn chặn câu khác; thêm file **trong lúc warm-up** thì mất 49 s.

## Thời gian (Cost wall)

| Repo / câu | Lần đầu | Ấm | Vòng 3 |
| --- | --- | --- | --- |
| hermes, `now` depth 1 (khởi động lạnh) | 21.713 ms; lần lạnh sau 15.603 / 14.937 ms; CPU ~110 s; 547 MB | 11 ms | 58.017 ms / 625 MB |
| hermes, `BasePlatformAdapter.send_image` depth 1 | 43 ms | 13–14 ms | 681–1.253 ms mỗi lần |
| hermes, `BasePlatformAdapter.__init__` depth 1 | 135 ms | — | 1.243 ms |
| hermes, `now` không giới hạn | 2.252 ms, 640 KB | — | 4,3 s |
| hermes, `send_image` không giới hạn | 1.002 ms, 240 KB | — | 19,7 s |
| hermes, `get_hermes_home` không giới hạn | 72.489 ms (lần 2: 59.142 ms), 51 MB output, 1.691 MB RAM | — | 212,6 s |
| hcopy: thêm 1 file / xoá 1 file rồi hỏi ngay | 8 ms (client 187 ms) / 8 ms (client 186 ms) | — | 61.857 / 57.190 ms |
| hcopy: thêm 1 file **trong lúc warm-up** rồi hỏi | **49.257 ms**, CPU 86,9 s (lạnh bình thường 12.960 ms) | — | (chưa đo) |
| oh-my-pi, đích `.py` đầu tiên | 11.303 ms, **582 MB** (Python driver chỉ 43 MB) | 5–9 ms | — |
| codex, đích `.py` đầu tiên | 1.551 ms, 260 MB | 5–6 ms | — |
| fx5 | 59 ms | 0–2 ms | 1 ms |

Không có `rust-analyzer` nào chạy sau các câu hỏi ở oh-my-pi và codex (`pgrep` rc=1). oh-my-pi vẫn bật `node driver.mjs` (286 MB) và `codetrace driver-lsp` (275 MB) cho một câu hỏi Python.

## Bảng đích

Hình thức: `callers <đích> --depth 1 --compact`. "rg" là đếm/đọc tay kết quả `rg`.

### hermes-agent (15 đích)

| # | Đích | Dạng khó | Cost | ct | rg / sự thật | Kết luận |
| --- | --- | --- | --- | --- | --- | --- |
| H1 | `gateway/platforms/base.py#BasePlatformAdapter.send_image` | `super()` ~30 lớp con, `super(DiscordMediaMixin, self)` trong lambda | 13–43 ms | 10 exact, 1 likely (`adapter_media.py:421`), 1 unresolved | Khớp rg 100%. 6 dòng `self.send_image` ở lớp có override (simplex, google_chat, teams, matrix, bluebubbles, telegram) bị loại đúng | **Hơn** |
| H2 | `#BasePlatformAdapter.send_image_file` | `super(TelegramAdapter, self)` trong hàm lồng | 38 ms | 4 exact + 5 unresolved | Đúng, kể cả `telegram/adapter.py:5323` | Hơn |
| H3 | `#BasePlatformAdapter.__init__` | constructor, nhiều static call `BasePlatformAdapter.x()` | 135 ms | 33 exact | 32 `super().__init__` + 1 `WireAdapter(...)`. Không còn dòng static/classmethod | Hơn (vòng 3: 23/55 sai) |
| H4 | `#BasePlatformAdapter.connect` | `@abstractmethod` | 33 ms | 26 unresolved | Đúng là chỉ gọi qua `adapter.connect()`. Có nhiễu stdlib: `socket.socket.connect`, `sock.connect(...)` (~10/26) | Ngang |
| H5 | `plugins/platforms/telegram/adapter.py#TelegramAdapter.send` | override, base gọi `self.send(...)` | 27 ms | 1 exact, 8 likely (đúng 8 `self.send` trong `base.py`), 143 unresolved | rg 8+1. Unresolved là mọi `.send(` (websocket…) | Hơn |
| H6 | `agent/delegation_context.py#delegated_child_subprocess_env` | 2 `@overload` + bản thật | 15 ms | 10 exact | Đủ. Header vẫn trỏ `:166` (stub), `locate :182` trỏ `:173`: hai dấu vân tay khác nhau cho cùng symbol | Ngang (nhẹ) |
| H7 | `agent/delegation_context.py:182` (callers theo dòng) | thân hàm thật sau overload | 12 ms | 10 exact | Vòng 3: 0 caller | **Đã sửa** |
| H8 | `tools/process_registry.py#ProcessSession.__post_init__` | dataclass | 25 ms | 4 dòng (5 chỗ tạo) | Đủ | Đã sửa |
| H9 | `tools/yuanbao_tools.py#_yb_tool.deco.handler` | hàm lồng tên phổ biến | 14 ms | 1 exact + 4 unresolved (`console_engine.py:208–216`, `self.handler` là trường dataclass của `_CliSurface`) | Hàm lồng không gọi được qua thuộc tính | Nhiễu giảm 12 → 4, còn |
| H10 | `tools/delegate_tool_progress.py#_ChildProgressRelay.__call__` | đối tượng gọi được, truyền làm callback | 169 ms | **3.492 unresolved, 357 KB** | Caller thật (`_safe_progress: cb(...)`, `:36`) nằm lẫn trong đó | **Kém** (nhiễu) |
| H11 | `hermes_state.py#SessionDB.__enter__` | `with SessionDB(...) as db:` | 93 ms | 4 exact đúng + **2.072 unresolved** (mọi `with x.lock:`) | rg `with SessionDB\(`: đúng 4 | **Kém** (nhiễu) |
| H12 | `hermes_cli/pty_bridge.py#PtyBridge.__enter__` | không ai `with` | 88 ms | 0 exact + 2.072 unresolved | rg: 0 | Kém |
| H13 | `hermes_cli/commands.py#__getattr__` | PEP 562, `from hermes_cli.commands import COMMANDS` | 14 ms | 0 | ≥5 chỗ import `COMMANDS`/`COMMANDS_BY_CATEGORY` (`cli.py:1338`, `commands_completion.py:19`…) | Kém (bỏ sót im lặng) |
| H14 | `tools/registry.py#ToolRegistry.get_entry` | instance cấp module `registry = ToolRegistry()` | 13 ms | 2 exact + 1 unresolved (`catalog.get_entry`, object khác) | rg 14 dòng, 11 là `ToolRegistry` (import `registry`/`registry as _tool_registry`). **Thiếu 9:** `tool_search_catalog.py:68`, `mcp_tool_agent.py:220`, `anthropic.py:21`, `plugins.py:479,2250,2256`, `plugins_cmd.py:922`, `web_server_dashboard.py:657`, `evals/.../worker.py:179` | **Kém (nặng)** |
| H15 | `tools/process_registry.py#ProcessRegistry.list_sessions` | như H14 | 14 ms | 5 unresolved (3 là method khác cùng tên) | rg 8. **Thiếu 6:** `tui_gateway/server.py:3511`, `cli_process_dock.py:50`, `cli_commands_session_tools.py:105,120`, `goals.py:1034`, `client_lifecycle.py:113` | **Kém (nặng)** |
| H16 | `gateway/browser_control_broker.py#BrowserControlBroker.scope_for_session` | `broker = get_browser_control_broker(); broker.scope_for_session()` | 11 ms | 1 unresolved | rg 3. **Thiếu** `browser_extension_router.py:59`, `methods_browser_control.py:117` | **Kém (nặng)** |
| H17 | `hermes_constants.py#get_hermes_home` | 586 caller | 154 ms | 586 exact, 5 unresolved | Đúng | Hơn |
| H18 | `gateway/hosted_room_peer.py#GatewayRoomCatalog.from_mapping` | classmethod, cùng tên ở class khác | 16 ms | 5 exact, 4 unresolved | Như vòng 3: `hosted_room_peer.py:273` (cùng file, chắc chắn) vẫn unresolved; 3 dòng `X.from_mapping` của class khác vẫn bị liệt kê | Ngang |

### oh-my-pi (6 đích, chỉ `.py`)

| # | Đích | Dạng khó | Cost | ct | rg | Kết luận |
| --- | --- | --- | --- | --- | --- | --- |
| O1 | `python/robomp/src/db.py#issue_key` | `package-dir = {"robomp" = "src"}`, `from robomp.db import issue_key` | 11.303 ms lạnh, 5 ms ấm | **0 caller** (cả `--tests`), rc=0 | 14 ngoài test (`tasks.py:489,584,647,840,987,1055`, `github_events.py:212…`, `worker.py:492`…) + ~60 test | **Kém (nặng nhất)** |
| O2 | `python/robomp/src/config.py#Settings` | như O1 | 5 ms | 2 dòng cùng file | ngoài test 1 (đúng), test ~60 (sót cả) | Kém với `--tests` |
| O3 | `packages/snapcompact/research/squad.py#build_flow` | `import squad` trong script kề | 9 ms | 43 exact | rg 44 dòng / 43 hàm | **Hơn** |
| O4 | `sdk/python/omp-rpc/src/omp_rpc/client.py#RpcClient._command` | hook do lớp cha sinh mã `WireClient` gọi | 8 ms | 4 exact + 60 likely | rg 60 trong `_wire.py` + 4 | Hơn |
| O5 | `#RpcClient.__enter__` | `with RpcClient(...)` qua re-export `from omp_rpc import` | 8 ms | 4 exact đúng + 117 unresolved | rg 4 | Ngang (nhiễu) |
| O6 | `#RpcClient.prompt_and_wait` | `with ... as client: client.prompt_and_wait()` | 6 ms | 1 exact (`rate-edit-tool.py:1400`), 26 unresolved | Đúng. `worker.py:321` (client là tham số) unresolved | Hơn |

### codex (5 đích, chỉ `.py`)

| # | Đích | Dạng khó | Cost | ct | rg | Kết luận |
| --- | --- | --- | --- | --- | --- | --- |
| C1 | `sdk/python/src/openai_codex/retry.py#retry_on_overload` | re-export `__init__`, ví dụ ngoài `src/` | 1.551 ms lạnh, 5 ms | 2 exact | 2 lời gọi | Ngang |
| C2 | `_message_router.py#MessageRouter.subscribe_turn` | `self._router = MessageRouter()` | 5 ms | 1 exact, 1 unresolved | 2 | Ngang |
| C3 | `_message_router.py#_TurnSubscription.__init__` | constructor nội bộ | 6 ms | 2 exact | 2 | Ngang |
| C4 | `scripts/codex_package/targets.py#default_target` | `from .targets`, test `from codex_package.targets` (root `scripts/pyproject.toml`) | 5 ms | 2 exact | 2 | Ngang |
| C5 | `third_party/voice/runtime.py#required_library_paths` | `from runtime import` (thư mục script) | 5 ms | 5 exact | 5 | Ngang, nhưng `test_assemble_package.py:61` (`for name in required_library_paths(t):`) ghi nhãn **reference** dù là lời gọi |

### fx5 (repo mẫu, sự thật lúc chạy)

| Đích | ct | Sự thật | Kết luận |
| --- | --- | --- | --- |
| `Cart.__init__` | 6 exact (gồm `SpecialCart()`) | Thiếu `core.py:27 cls()` trong `@classmethod` | Còn sót 1 |
| `Cart.__call__` / `__enter__` / `__exit__` / `__iter__` / `__getattr__` / `__len__` | `cart(3)`, `with Cart([1])`, `for item in cart`, `cart.item_0` likely, `len(cart)` | Thiếu `core.py:74 len(self)`. `__call__` thêm 4 unresolved nhiễu (`func(*args)`, `save(1)` là tham số) | Đã sửa phần lớn |
| `Cart.add` | 11 dòng; `getattr(cart,"add")` có | `Cart().add(1)` (`hard.py:98`) chắc chắn nhưng unresolved | Tốt |
| `Cart.checkout` (`@retry(times=3)`), `Cart.summary` (`@timed`) | còn "Affected entry points: @retry(times=3)", "@timed" | Decorator chỉ bọc, không đăng ký | Entry point giả còn |
| `A1.hello` / `C1.hello` (MRO `D1(B1, C1)`) | `D1.greet` exact ở A1, 0 ở C1 | Chạy thật `'a1'` | **Đã sửa** |
| `Right.setup` / `Base.setup` (kim cương) | `Left.setup:11` likely ở Right; exact ở Base | Đúng | Đã sửa |
| `Repo.__init__`, `Service.__init__` qua `storage.` | `app.py:30`, `app.py:28` exact | Đúng | Đã sửa |
| `Money.__post_init__`, `Money` | 3 chỗ tạo; không còn entry `@dataclass(frozen=True)` | Đúng | Đã sửa |
| `Vec.__eq__/__add__/__getitem__/__contains__/__bool__/__hash__` | 0 cả 6, không cảnh báo | `operators()` chạy cả 6 (`a + b`, `a == b`, `a[0]`, `1 in a`, `if a`, `hash(a)`) | Bỏ sót im lặng (toán tử) |
| `Meta.__call__` | 4 unresolved nhiễu | `EchoPlugin()` gọi metaclass `__call__` | Sót + nhiễu |
| `Plugin.__init_subclass__`, `Desc.__get__`, `Desc.__set_name__` | 0 | `class EchoPlugin(Plugin)`, `h.value`, `value = Desc()` | Sót im lặng (hiếm) |
| `Plugin.run` | `EchoPlugin().run()` unresolved | Chắc chắn | Nhãn thấp |
| `parse` (2 `@overload`), `from .. import hard`, `from ..hard import parse as p` | 2 hàm, 4 dòng exact | Đúng | Tốt |
| `_process_int` (`@process.register(int)`) | entry point đúng | Đúng | Tốt |
| `repo.py#save` với `def shadow(save): save(1)` | 0 | Đúng (tham số che tên) | Tốt |

## locate

| Vị trí | Dạng | Kết quả | Đúng? |
| --- | --- | --- | --- |
| hermes `agent/delegation_context.py:182` | thân hàm thật sau 2 `@overload` | `#delegated_child_subprocess_env :173` | Đúng (vòng 3 sai) |
| hermes `hermes_cli/backup.py:201` | `_lock_op` thứ hai trong `else:` | `#_backup_operation_lock._lock_op :200` | Đúng (vòng 3 sai) |
| hermes `agent/context_compressor.py:2207` | thân `@context_length.setter` | `#ContextCompressor.context_length :2205` | Đúng (vòng 3 sai) |
| hermes `tools/yuanbao_tools.py:46` / `:43` | `async def handler` lồng 3 tầng / dòng `@functools.wraps` | `#_yb_tool.deco.handler` / `#_yb_tool.deco` | Đúng |
| hermes `plugins/platforms/discord/adapter_media.py:421` | lambda trong đối số | `#DiscordMediaMixin.send_image` | Đúng |
| hermes `plugins/platforms/discord/adapter.py:6409` | class lồng trong hàm | `#_define_discord_view_classes._HermesView.__init__` | Đúng |
| hermes `hermes_state_holders.py:336`, `:340` | class lồng trong hàm (ctypes) | `#_rm_ctypes._RmUniqueProcess` / `._RmProcessInfo` | Đúng |
| hermes `gateway/platforms/base.py:2727` | docstring của `@abstractmethod async def` | `#BasePlatformAdapter.connect` | Đúng |
| oh-my-pi `python/robomp/src/server.py:125` / `:112` | lambda trong `sort(key=)` của `async def` / dòng tham số | `#_IssueBrowseCache.get_or_fetch` | Đúng |
| oh-my-pi `sdk/.../client.py:218` | hàm lồng | `#_terminate_process_group._leader_exited` | Đúng |
| oh-my-pi `sdk/.../client.py:581` / `:579` | thân `@property` / dòng `@property` | `#RpcClient.stderr` / `#RpcClient` | Đúng / chấp nhận |
| fx5 `shop/core.py:23` | thân `@total.setter` | `#Cart.total :22` | Đúng (vòng 3 sai) |
| fx5 `shop/nested.py:30` | nhánh `else: def platform_name` | `#platform_name :29` | Đúng (vòng 3 sai) |
| fx5 `shop/hard.py:75`, `:107`, `:120`, `:85`, `:32` | overload thật, closure, `match`, `@register`, metaclass | đúng symbol tương ứng | Đúng |
| fx5 `shop/dyn.py:12` sau khi sửa thành `if/else: def double` | nhánh `else` | `#double :11` | Đúng |
| fx5 `shop/hard.py:999`, `shop/nope.py:3` | quá cuối file / file không có | `#(module)` rc=0 / in lại đầu vào rc=0 | Như vòng 3 (nhẹ) |

## Kiểm lại lỗi vòng 3

| Lỗi vòng 3 | Bây giờ | Bằng chứng |
| --- | --- | --- |
| Thêm/xoá 1 file ở repo lớn mất 57–62 s | **Hết** (8 ms, client 187 ms). **Còn** khi thêm trong lúc warm-up: 49 s | hcopy `e1-add`, `e2-rm`, `w1` |
| Câu hỏi lớn chặn câu khác | **Còn.** Trong 72 s `get_hermes_home` chạy, `locate` và `callers` depth 1 đều hết 30 s | `hm-ghh-full` |
| Client bỏ cuộc mà daemon vẫn tính | **Hết** (1 lần thử). Bỏ ở giây 20 thì `locate` trả lời sau 2,6 s (vòng 3: 88 s); chạy lại mất 59 s, tức phép tính dở đã bị bỏ | |
| Định nghĩa trùng (`@overload`, setter, `if/else`) làm `locate` sai | **Hết** (5/5 vị trí cũ đúng). Còn: header `callers` theo tên vẫn trỏ stub đầu | H6/H7 |
| MRO đa kế thừa | **Hết** | fx5 `A1.hello` |
| `Cls.static()` tính là gọi `__init__` | **Hết** (hermes 33/33 đúng) | H3 |
| Dunder ngầm | **Hết một phần**: `with`/`for`/`len(x)`/`x()`/`__getattr__`/`__post_init__` có caller chắc chắn. Còn sót `len(self)`, toán tử, metaclass, `__init_subclass__`, PEP 562. **Mới:** nhiễu hàng nghìn dòng | H10–H13 |
| Constructor thừa kế | **Hết** (`SpecialCart()`). Còn `cls()` | fx5 |
| `from . import repo as storage` | **Hết** | fx5 |
| `getattr(obj, "tên")()` | **Hết** (unresolved) | fx5 |
| `super(Mixin, self)` | **Hết** (`likely`) | H1 |
| Decorator dạng gọi thành entry point | **Hết một phần**: `wraps`/`dataclass(...)` hết; `@retry(times=3)`, `@timed` còn | fx5 |
| Hàm lồng tên phổ biến kéo `x.handler` | **Hết một phần** (12 → 4) | H9 |
| `BasePlatformAdapter.*` 0,7–1,25 s mỗi lần | **Hết** (13–135 ms) | H1, H3 |
| Tên trần gõ sai ra 0 caller rc=0 | **Hết** ("Not found", rc=1) | |

## Lỗi mới theo mức nặng

### Nặng

1. **`package-dir` trong `pyproject.toml` không được hiểu: cả một gói bị bỏ im lặng.**
   - Bằng chứng: oh-my-pi `python/robomp/pyproject.toml:37` `package-dir = { "robomp" = "src" }`. `src/` có 101 dòng `from robomp…`, tất cả bị bỏ. `db.py#issue_key` ra 0 caller, rg ra 14 ngoài test + ~60 trong test.
   - Nguyên nhân: `Modules.__init__` (`driver.py:42–55`) chỉ lấy root là thư mục chứa `pyproject.toml` hoặc thư mục `src` (`python/robomp/src` → tên `config`, `db`), không bao giờ ra `robomp.db`. Import không giải được thành `("package",)` (`:166`); `visit_Name`/`visit_Attribute` (`:400–434`) bỏ qua `package` mà không ghi `global`/`member`, nên dòng gọi không còn cả ở dạng `unresolved`.
   - Đề xuất:
     - Đọc `[tool.setuptools] package-dir` (cả `packages.find where=`, `[tool.hatch…] sources`/`packages`, `[tool.poetry] packages` nếu muốn) để thêm ánh xạ tên → thư mục.
     - Tối thiểu: import trỏ vào tên gói không tìm thấy nhưng có `def`/`class` trùng tên trong repo thì ghi ref `global`/`member` để ra `unresolved`, không bỏ im lặng.
2. **Method gọi trên đối tượng lấy từ hàm import hoặc instance cấp module import bị bỏ im lặng.**
   - Bằng chứng:
     - H14 thiếu 9/13 dòng, H15 thiếu 6/8, H16 thiếu 2/3.
     - fx5 `shop/edit2.py:6` (`cart = make_cart(1); cart.add(2)`) không có ở `Cart.add` mà lại ra `[reference]` của `make_cart`. Dạng `other = Cart.from_list(...); other.add()` thì vẫn ra `unresolved`.
   - Nguyên nhân:
     - `member_of_construction` (`driver.py:476–478`) coi mọi tên import (`binding[2] != "*"`) là class, ghi `to=import, name=make_cart, member=add`. `visit_Attribute` (`:430–432`) cũng ghi `to=import, name=registry, member=get_entry` cho instance cấp module.
     - `compute_callers` chỉ khớp ref `import` với alias `(module, ClassName, member)` (`:880–892`). `by_name` không chứa ref `import` (`:574–577`), nên ref này không bao giờ ra.
   - Đề xuất: lúc trả lời, nếu `(module, name)` không phân giải ra một class (`resolve_export` + `facts.methods`) thì coi ref như `member` tên `member` (vào `by_name`, ra `unresolved`). Tốt hơn: ghi `x = Cls()` cấp module thành export kiểu instance để ra `exact`.
3. **Nhiễu dunder ngầm hàng nghìn dòng ở depth 1.**
   - Bằng chứng:
     - `__enter__`/`__exit__` của bất kỳ class nào: 2.072 dòng (`with self._lock:`…).
     - `__call__`: 3.492 dòng (mọi lời gọi tham số/biến cục bộ).
     - Cũng xuất hiện ở fx5 (`deco.py:11 func(*args)`) và oh-my-pi (117 dòng).
     - Agent nhận 250–360 KB; 4 dòng exact đúng bị chôn.
   - Nguyên nhân: `visit_With` (`driver.py:489–497`) và `visit_Call` (`:515–516`) gọi `implicit(..., unknown=True)`, rồi `compute_callers` (`:900–901`) đưa mọi `member` cùng tên vào `unresolved`.
   - Đề xuất: với dunder, khi đã có caller chắc chắn hoặc số dòng unknown quá lớn thì gom thành một dòng đếm ("2.072 `with`/lời gọi trên đối tượng không rõ kiểu, không liệt kê"). Hoặc chỉ ghi `unknown` khi biến có ràng buộc tới class của repo (type hint, tham số mặc định).
4. **Thêm/xoá file trong lúc warm-up làm mất toàn bộ warm-up: 49 s thay vì 13 s.**
   - Bằng chứng: hcopy `w0`/`w1`, CPU 86,9 s.
   - Nguyên nhân: `sync` tăng `self.layout` (`driver.py:655`), `finish_warm_up` bỏ mọi kết quả khác layout (`:626–627`), rồi `read_all` đọc tuần tự 8.420 file (`:636–638`).
   - Đề xuất: vẫn nhận kết quả warm-up rồi chỉ đọc lại các file `spelling(...)` của file vừa thêm/xoá, giống nhánh đã có ở `:656–658`.
5. **Câu hỏi lớn vẫn chặn mọi câu khác của repo.** `get_hermes_home` không giới hạn chạy 59–72 s (51 MB output, 1,69 GB). Trong lúc đó `locate` cũng hết 30 s. Driver đọc stdin tuần tự (`driver.py:948–959`), nên mọi câu xếp hàng. Đề xuất: đặt trần mặc định `--max-nodes` cho câu không có `--depth` từ MCP, hoặc trả lời `locate` ở lõi/luồng riêng.

### Vừa

6. **Nhãn sai `reference` cho lời gọi khi `for`/`with` lặp trên kết quả hàm import.** `for name in required_library_paths(t):` (codex `third_party/voice/test_assemble_package.py:61`; fx5 `edit1.py:6,8`) ra `[reference]`. Nguyên nhân: `implicit` ghi ref `import … member="__iter__"` cùng dòng; nhánh `item is None and reference.get("member")` (`driver.py:889–890`) đổi thành `reference`, và dòng `call` thật bị gộp mất.
7. **PEP 562 `__getattr__` cấp module luôn 0** (H13). Có thể nối: tên import từ module đó mà module không định nghĩa → caller `likely` của `#__getattr__`.
8. **`self.x` là trường dữ liệu của chính class vẫn được ghép với method/hàm lồng cùng tên khác** (H9). `driver.py:902–905` chỉ kiểm `first_definer`, không kiểm `holds` (thuộc tính, trường dataclass); và hàm lồng (`_yb_tool.deco.handler`, owner không phải class) vẫn đi nhánh method.
9. **Gốc `stdlib` vẫn đổ vào `unresolved`:** `socket.socket.connect`, `sock.connect` ở `BasePlatformAdapter.connect` (H4). Chuỗi bắt đầu bằng import ngoài repo (`("package",)`) chắc chắn không phải method của repo.
10. **Câu Python ở repo đa ngôn ngữ bật mọi driver:** oh-my-pi lần đầu 11,3 s / 582 MB, trong đó Python driver chỉ 43 MB, còn lại `node driver.mjs` 286 MB và `driver-lsp` 275 MB (không có rust-analyzer).
11. **Gọi ngay sau `codetrace stop` ở repo lớn thì lỗi:** `Connection reset by peer (os error 104)`, rc=2, 49 ms. Lặp lại 2/2 lần trên hermes; chờ 3 s thì ổn; repo nhỏ không bị.

### Nhẹ

12. Vẫn sót: `len(self)` (`core.py:74`), `cls()` ở `Cart.__init__`, dunder toán tử (`+ == [] in bool`), metaclass `__call__`, `__init_subclass__`, descriptor `__get__/__set_name__`. Vẫn im lặng; nên in một dòng "method gọi ngầm qua toán tử, chưa lần".
13. Nhãn thấp: `Cls().m()` viết liền (`hard.py:98`, `:133`) và `Cls.m()` cùng file (`hosted_room_peer.py:273`) vẫn `unresolved`. `self._router = MessageRouter()` rồi `self._router.x()` cũng `unresolved`.
14. Decorator chỉ bọc (`@retry(times=3)`, `@timed`) vẫn ra "Affected entry points".
15. Header `callers` theo tên trỏ stub `@overload` đầu (`:166#334e`), còn `locate` trỏ bản thật (`:173#a492`): cùng symbol, hai dấu vân tay.
16. `locate` file không tồn tại vẫn in lại đầu vào, rc=0. Dòng vượt cuối file ra `(module)`.

## Ghi chú

- Bản sao: `.../scratchpad/py5/fx5` (git riêng), `.../scratchpad/py3/hcopy` (đã xoá lại các file thử `new_probe_mod5/6.py`).
- Cuối buổi đã `codetrace stop` hermes-agent, oh-my-pi, codex, fx5, hcopy. Không stop daemon `backend-copy` của phiên khác; lúc kết thúc `codetrace status` ra `Servers running: 0`, tức nó đã tự tắt (có thể do idle).
