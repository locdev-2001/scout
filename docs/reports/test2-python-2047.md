# Test độc lập driver Python của codetrace (lần 2)

Ngày 25/10/2026, 20:47. CLI `codetrace` trên PATH, daemon `codetrace serve` đã ấm (không kill). Hai repo thật (chỉ đọc):

- workspace `/home/robertnguyen/www/easy-ai-dev`: driver thấy 46 file `.py` (generators dùng `from lib import *`, `scripts/worktree-clean`).
- backend `repos/easy-ai-platform-backend`: 99 file `.py` (`plugins/*/server.py`, `.opencode/skills/*/scripts`, `scripts/workspace_agent_live.py`).

Hai repo thật không có import tương đối nào (`rg '^\s*from \.+' --hidden` ra rỗng), và `import a.b as c` chỉ dùng cho thư viện ngoài. Vì vậy tôi dựng thêm một repo mẫu **ngoài repo**, trong scratchpad: `/tmp/claude-1000/-home-robertnguyen-www-easy-ai-dev/a6c5ca1b-1368-4a00-910f-bbe94707325a/scratchpad/fx` (git init, 17 file), rồi gọi bằng `--repo`. Cách đối chiếu: với mỗi đích, chạy `rg -n '\b<tên>\b'` trên toàn repo, so từng dòng với output `--compact` (dùng script tách `file:dòng`), đọc tay những dòng lệch.

## Kết luận nhanh

- **Phần làm tốt:** `from lib import *` (31 file, 226 dòng gọi `pct`, khớp `rg` 100%; `ads.py` có `def pct` riêng và được loại ra đúng). Import theo thư mục script (`from removal import ...`), hàm truyền làm callback (`pool.map(inspect, …)`, `pool.submit(remove, …)`), decorator FastAPI là điểm vào, `if __name__ == "__main__"`, import tương đối `.`/`..`/`from . import x`/alias `as`, gốc `src/`, `import a.b as c` rồi `c.f()`, hàm lồng: đều đúng. Riêng dạng không alias `import a.b` rồi `a.b.f()` thì bị sót (đích 12).
- **Tốc độ:** mọi lệnh khi ấm có `Cost` 0–20 ms wall. Đạt mục tiêu dưới 100 ms. Riêng repo backend, mỗi lệnh báo khoảng 1 GB bộ nhớ (tính chung với driver PHP/JS).
- **Lỗi nặng nhất:** ở repo nhiều ngôn ngữ (backend), `locate x.py:N` và `callers x.py:N` lấy câu trả lời của driver PHP (một symbol cấp file, không có `#`), còn câu trả lời đúng của driver Python bị bỏ. Kết quả `callers` ra **0 caller, không có cảnh báo**.
- **Bỏ sót im lặng trên code thật:** `self.m()` gọi trong hàm lồng hoặc lambda bên trong method (`batch_resize.py:179`, `workspace_agent_live.py:393`), và lời gọi constructor `Cls(...)` khi tra `Cls.__init__`.
- **`exact` sai (thấy trên repo mẫu):** tham số lambda trùng tên với hàm module; override ở lớp trung gian; `class Sub(mod.Base)` bị tính là caller của mọi method của `Base`; import theo thư mục script bị resolve về file trùng tên ở gốc repo.

## Bảng các đích

| # | Đích | Kiểu khó | Cost (ấm) | Kết quả codetrace | Đối chiếu độc lập | Lỗi |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `plans/261021-insights-reports-revamp/generators/lib.py#pct` | `from lib import *` ở 31 file; `ads.py:37` có `def pct` riêng | 184 ms (lần đầu), sau đó 1 ms | 31 exact, 226 dòng | `rg '\bpct\('`: 233 dòng. Lệch 7 dòng đều nằm trong `ads.py` (`:75,77,79,81,134,166,176`), gọi `pct` của chính `ads.py:37` → loại ra là **đúng** | Không |
| 2 | `generators/rd.py#short`, `#total`, `#ch`, `#daily` | `from rd import short` (`website.py:5`) và `from rd import *` (4 file); `total`/`daily`/`ch` trùng tên biến cục bộ ở file khác | 1 ms mỗi lệnh | `short`: `products.py:22` (trong lambda), `website.py:54`. `total`: `orders_revenue.py:10–13`, `pricing.py:8–10`. `ch`, `daily`: đủ | `rg` chỉ trong 5 file import `rd` khớp từng dòng. Các file không import `rd` (`rd_prep.py`, `campaign.py`…) dùng biến cùng tên và được loại ra đúng | Không |
| 3 | `scripts/worktree-clean/removal.py#run`, `#inspect`, `#remove`, `#processes`, `#dependency_in_use`, `#stop`, `#recheck` | Thư mục script là gốc import (`cli.py:14 from removal import …`); callback `pool.map(inspect, paths)` `cli.py:133`, `pool.submit(remove, …)` `cli.py:169` | 1 ms | `run`: 6 caller, 8 dòng (`cli.py:49,141`; `removal.py:19,42,47,63,144,152`). `inspect`: `cli.py:133` [reference] + `removal.py:122` | Khớp `rg` (các dòng `subprocess.run`, `status=` ở `rg` là thuộc tính/khóa dict, loại đúng) | Không |
| 4 | `scripts/worktree-clean/cli.py#main` | `if __name__ == '__main__'` | 2 ms | `cli.py:180` exact; điểm vào `@__main__ (cli.py:178)` | Đúng | Không |
| 5 | `plugins/sentiment-analyze/server.py#_tokens`, `#encode_conversation`, `#score_conversation`, `#normalize_conversation`, `#overall_from_score`, `#analyze`, `#ContractError` | Decorator `@app.get/@app.post` là điểm vào | 8–10 ms | Ví dụ `_tokens` → `:73,74,82,83`; đi lên tới `@app.get('/health') :161`, `@app.post('/analyze') :182`. `ContractError` → `:76,98` (raise), `:203` (except) | Khớp `rg` từng dòng | Không |
| 6 | `.opencode/skills/mcp-builder/scripts/connections.py#MCPConnectionStdio._create_context` | Override: lớp cha gọi `self._create_context()` (`:30`) | 10 ms | `:30` likely | Đúng | Không |
| 7 | `connections.py#MCPConnection._create_context` (`@abstractmethod`) | Decorator không phải đăng ký | 9 ms | `:30` exact và **"Affected entry points: @abstractmethod (:20)"** | `@abstractmethod`, `@classmethod` không phải điểm vào | Nhiễu: điểm vào giả |
| 8 | `connections.py#MCPConnection.call_tool`, `#list_tools` | Gọi qua biến không rõ kiểu (`evaluation.py:124 connection.call_tool`, `:238`) | 8–9 ms | 2 unresolved mỗi đích, gồm cả `self.session.call_tool` `:69` | Hợp lý | Không |
| 9 | `connections.py#MCPConnectionStdio.__init__` | Constructor `MCPConnectionStdio(...)` `:138` | 9 ms | 4 unresolved: các dòng `super().__init__()` `:77,92,104` và `generate_review.py:330` | Caller thật `:138` không có ở đây. Tra symbol lớp `#MCPConnectionStdio` thì ra `:138` exact, nghĩa là lời gọi `Cls(...)` được gán cho lớp chứ không cho `Cls.__init__`; đây là chỗ mô hình còn hở, ai tra `__init__` sẽ không thấy. Bốn dòng `super().__init__()` thật ra gọi lớp cha (`MCPConnection.__init__`, `BaseHTTPRequestHandler.__init__`), không gọi đích này, nên là nhiễu. Ngược lại, khi tra `MCPConnection.__init__` thì ba dòng `:77,92,104` là caller thật (lớp con gọi lớp cha đã biết, cùng file), nhưng chỉ được gắn `unresolved` | Thiếu với `__init__` + nhiễu + độ tin cậy thấp hơn mức có thể |
| 10 | `.opencode/skills/media-processing/scripts/batch_resize.py#ImageResizer.resize_image` | `self.resize_image(...)` trong hàm lồng `batch_resize.process_image` (`:164`), dòng `:179` | 8 ms | **0 caller** | `rg -n resize_image`: `:179` | **Bỏ sót im lặng** |
| 11 | `scripts/workspace_agent_live.py#WorkspaceAgentApp.command` | `self.command(...)` trong method (`:325`) và trong lambda (`:393`) | 9 ms | `:325` exact; 19 unresolved | `:393 lambda: asyncio.create_task(self.command("/quit"))` **thiếu**. 19 unresolved phần lớn là đọc thuộc tính `args.command ==` và cả phép gán `self.command = command` (`connections.py:78`) | Bỏ sót im lặng + nhiễu |
| 12 | Repo mẫu `pkg/core.py#helper` | `from ..core import helper`, re-export qua `pkg/__init__.py`, `from pkg.core import *`, `import pkg.core`, tham số lambda trùng tên | 0–1 ms | `deep.py:8`, `main.py:10,11`, `core.py:60` (callback), **`core.py:42` exact** | `core.py:42 lambda helper: helper(3)` gọi tham số chứ không gọi hàm → **exact sai**. `main.py:9 pkg.core.helper(2)` (sau `import pkg.core`) **thiếu, không báo** | Exact sai + bỏ sót im lặng |
| 13 | Repo mẫu `pkg/sub/deep.py#deep_fn` | `import pkg.sub.deep as deepmod`; import cục bộ trong hàm | 1 ms | `main.py:8` | `core.py:47–48` (`from pkg.sub.deep import deep_fn` trong thân hàm, rồi `deep_fn()`) **thiếu, không báo** | Bỏ sót im lặng |
| 14 | Repo mẫu `pkg/sub/sibling.py#sib_fn`, `proj/src/app/svc.py#svc_fn` | `from . import sibling`, `from .sibling import sib_fn as renamed`, gốc `src/`, `from .svc import … as again` | 1 ms | Đủ, exact | Đúng | Không |
| 15 | Repo mẫu `tools/util.py#tool_fn` và `util.py#tool_fn` (trùng tên ở gốc repo) | `tools/cli.py: from util import tool_fn` | 1 ms | `tools/util.py`: 0 caller. `util.py`: `tools/cli.py:5` exact | Chạy `python tools/cli.py` thì Python đặt `tools/` đầu `sys.path`, nên `tools/util.py` mới đúng. Driver tra gốc repo trước thư mục script (`driver.py:76–83`) | Exact sai + bỏ sót (chỉ khi có file trùng tên ở gốc; hai repo thật hiện không có) |
| 16 | Repo mẫu `pkg/core.py#Base.area` (`Mid(Base)` override `area`, `Leaf(Mid)` gọi `self.area()`) | Override ở lớp trung gian | 1 ms | `core.py:35` exact cho `Base.area` | `Leaf` thừa kế `Mid.area`, nên `:35` không bao giờ chạy `Base.area` | Exact sai |
| 17 | Repo mẫu `pkg/core.py#Leaf.hook` (`Base.template` gọi `self.hook()`, chỉ `Leaf` định nghĩa `hook`, cùng file) | Template method | 1 ms | 0 caller | `core.py:17` thiếu. Khác file thì sẽ ra unresolved, cùng file thì bị loại vì điều kiện `file != path` (`driver.py:521`) | Bỏ sót im lặng |
| 18 | Repo mẫu `sub_attr.py#Sub.area`, `pkg/core.py#Base.report` / `#Base.area` (`class Sub(core.Base)`) | Lớp cha viết qua thuộc tính module | 1 ms | `Sub.area`: không có `core.py:14` (likely). `Base.report`, `Base.area`: `sub_attr.py:4 class Sub(core.Base)` **exact [reference]** | Dòng `:4` chỉ nhắc tới lớp, không gọi method nào | Bỏ sót im lặng + exact sai |
| 19 | Repo mẫu `chain_a.py#dumps` (`try: from fastjson import dumps / except ImportError: def dumps`) | Định nghĩa dự phòng | 1 ms | 0 caller | `chain_a.py:12 dumps(1)` thiếu | Bỏ sót im lặng |
| 20 | Repo mẫu `chain_c.py#chained` qua chuỗi `*` (`a` → `from chain_b import *` → `from chain_c import *`) | Star lồng | 1 ms | `chain_a.py:11` unresolved | Có báo, chỉ chưa exact | Chấp nhận được |

## locate

| Vị trí | Repo | Kết quả | Đúng? |
| --- | --- | --- | --- |
| Trong method: `connections.py:31` | backend | `connections.py` (không `#`, dòng 1) | **Sai**: phải là `#MCPConnection.__aenter__`. Gọi thẳng driver Python thì ra đúng |
| Trong hàm lồng: `batch_resize.py:179` | backend | `batch_resize.py` (dòng 1) | **Sai**: phải là `#ImageResizer.batch_resize.process_image`. Driver Python trả đúng |
| Cấp module: `plugins/sentiment-analyze/server.py:47` | backend | `server.py` (không `#(module)`) | Sai dạng |
| Trong method: repo mẫu `pkg/core.py:14` | mẫu | `#Base.report :13` | Đúng |
| Trong hàm lồng: `generators/lib.py:121` | workspace | `#table.row_js :120` | Đúng |
| Hàm lồng: repo mẫu `chain_a.py:17` | mẫu | `#outer.inner :16` | Đúng |
| Cấp module: `scripts/worktree-clean/cli.py:180` | workspace | `#(module)` | Đúng |

Nguyên nhân lỗi `locate` ở backend: `resolve_target` (`scripts/codetrace/src/trace.rs:35`) gửi `locate` cho mọi driver rồi lấy câu trả lời đầu tiên (`take(1)`). Driver PHP trả lời cho mọi file, kể cả `.py`: `FactStore::locate` (`drivers/php/fact-store.php:1156`) không lọc đuôi file, và khi không có scope thì `scopeRef` (`:1053`) trả về `{symbol: <path>, kind: 'file'}`. Hệ quả: `codetrace callers plugins/sentiment-analyze/server.py:200` ra "Direct callers (0)", không có cảnh báo, dù dòng 200 nằm trong `analyze`. Đây là lỗi của core hoặc driver PHP, nhưng người dùng Python ở backend gặp trực tiếp.

## Nguyên nhân trong `driver.py`

- `self.m()` trong hàm lồng hoặc lambda bên trong method: `enclosing_class()` (`driver.py:226–232`) gặp scope `function` của method thì trả `None`. Tham chiếu được ghi là `to="self", owner=None`, rồi bị loại vì cùng file (`:521`).
- Tham số lambda: `binding_of` (`:216`) dùng `key=None` của scope lambda, ra `own = name`. Nếu module có hàm cùng tên thì trả `("own", name)`.
- Import trong thân hàm: `declare` (`:161`) chỉ bind import khi `not prefix`. Trong hàm, tên đó là `local` (`:217`) nên không sinh reference.
- `import pkg.core` rồi `pkg.core.helper()`: `pkg` được bind tới `pkg/__init__.py` với `name="*"`. `visit_Attribute` bên ngoài thấy `value` là Attribute nên ghi `member helper`, sau đó `aliases` không khớp và nhánh unresolved chỉ áp cho key có dấu chấm (`:519`).
- Override ở lớp trung gian: `overrides` (`:493`) chỉ xét chính lớp con, không xét các lớp nằm giữa.
- `class Sub(core.Base)`: trong `callers`, nhánh `name == "*"` khớp `member == item.split(".")[0]` (`:508`), nên mọi tham chiếu `core.Base` thành caller exact của mọi method của `Base`. Còn `bases` ghi `("expression",)` (`:247`), nên `ancestors_of` và `subclasses_of` không đi qua được.
- Thứ tự tra module: `Modules.resolve` (`:76–83`) tra `by_name` (các gốc repo) trước thư mục của script. Python làm ngược lại.
- `try: from x import f / except ImportError: def f`: `module_bindings` (import) được ưu tiên hơn `declarations` (`:218–221`). Import ngoài cho ra `("package",)`, và `visit_Name` bỏ qua binding này.
- Decorator nào cũng thành điểm vào (`:158–159`), kể cả `@abstractmethod`, `@classmethod`, `@staticmethod`, `@property`.
- `visit_Attribute` không xét `ctx`, nên phép gán `self.x = …` cũng thành reference unresolved.

## Ghi chú về cách test

- Repo mẫu nằm ngoài repo, trong `/tmp/claude-1000/-home-robertnguyen-www-easy-ai-dev/a6c5ca1b-1368-4a00-910f-bbe94707325a/scratchpad/fx`.
- Một lần chạy `importlib` để in các gốc module của driver đã vô tình sinh ra `scripts/codetrace/drivers/python/__pycache__/` (mtime 20:43:06). Thư mục này đã được xóa. Trong repo không có thay đổi nào khác ngoài file báo cáo này.
