"""Python driver. Reads code with CPython's own `ast`, the same parser the interpreter uses, so it needs nothing
installed beyond the Python that runs it. What each file declares, imports and references is kept per file and
read again when the core reports the file changed.

Symbols are `path#key`: `format_price`, `Cart.add` (method), `load.inner` (function inside another),
`(module)` (top-level code). A reference is `exact` when the name is bound for certain: an import of the target's
module, a binding in the same file, `self.method()` in the class that defines it. A call through an object of
unknown origin (`cart.add()`) or a name bound nowhere is listed as `unresolved`, never dropped. A function under
a decorator (`@app.route(...)`) is called by whatever the decorator registers it with: that is its entry point.
"""

import ast
import builtins
import configparser
import hashlib
import json
import os
import pickle
import select
import subprocess
import sys
import tomllib
import zlib
from concurrent.futures import ProcessPoolExecutor

sys.dont_write_bytecode = True  # warm-up workers import this file: no cache files beside it
REPO = os.path.realpath(sys.argv[1])
BUILTINS = set(dir(builtins))
TEST_PARTS = ("tests/", "test/", "/tests/", "/test/")
PROJECT_MARKERS = ("pyproject.toml", "setup.py", "setup.cfg")


def list_files():
    output = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "-z", "--", "*.py"], cwd=REPO, capture_output=True, text=True).stdout
    return {path for path in output.split("\0") if path}


def stamp(path):
    """(modification time, size) of a repository file, None when it is gone."""
    try:
        status = os.stat(os.path.join(REPO, path))
    except OSError:
        return None
    return status.st_mtime_ns, status.st_size


def is_test(path):
    name = os.path.basename(path)
    return name.startswith("test_") or name.endswith("_test.py") or any(part in "/" + path for part in TEST_PARTS)


class Modules:
    """Which file a dotted module name means. Python searches its path roots in order; here the roots are the
    repository, every `src/` folder and every folder holding a project file, which is where a project's code is
    importable from when installed or run."""

    def __init__(self, files):
        self.files = files
        roots = {""}
        for path in files:
            parts = path.split("/")
            for depth in range(len(parts)):
                folder = "/".join(parts[:depth])
                if parts[depth] == "src" or any(os.path.exists(os.path.join(REPO, folder, marker)) for marker in PROJECT_MARKERS):
                    roots.add("/".join(parts[: depth + 1]) if parts[depth] == "src" else folder)
        self.roots = sorted(roots, key=len, reverse=True)
        self.package_dirs = declared_package_dirs({folder for path in files for folder in project_folders(path)})
        self.by_name = {}
        for path in files:
            for name in self.names_of(path):
                self.by_name.setdefault(name, path)

    def names_of(self, path):
        stem = path[:-3]
        if stem.endswith("/__init__"):
            stem = stem[: -len("/__init__")]
        names = []
        for folder, package in self.package_dirs:
            if stem == folder or stem.startswith(folder + "/"):
                names.append(package + stem[len(folder) :].replace("/", "."))
        for root in self.roots:
            if root == "" or stem.startswith(root + "/"):
                inside = stem[len(root) + 1 :] if root else stem
                if inside and inside != "__init__":
                    names.append(inside.replace("/", "."))
        return names

    def package_of(self, path):
        names = self.names_of(path)
        if not names:
            return ""
        name = names[0]
        return name if path.endswith("__init__.py") else name.rpartition(".")[0]

    def resolve(self, dotted, importer=None):
        """The file for a module name. A file outside any package is run as a script, and Python puts the
        script's own folder first on its path; a package's module imports from the project roots."""
        if not dotted:
            return None
        local = None
        if importer is not None:
            base = os.path.join(os.path.dirname(importer), *dotted.split("."))
            local = next((candidate for candidate in (base + ".py", os.path.join(base, "__init__.py")) if candidate in self.files), None)
            in_package = os.path.join(os.path.dirname(importer), "__init__.py") in self.files
            if local and not in_package:
                return local
        return self.by_name.get(dotted) or local


def project_folders(path):
    parts = path.split("/")[:-1]
    return ["/".join(parts[:depth]) for depth in range(len(parts) + 1)]


def declared_package_dirs(folders):
    """`package-dir = {"robomp" = "src"}` (setuptools, in pyproject.toml or setup.cfg): `robomp.x` is `src/x.py`."""
    found = []
    for folder in folders:
        base = os.path.join(REPO, folder)
        mapping = {}
        try:
            with open(os.path.join(base, "pyproject.toml"), "rb") as handle:
                mapping = tomllib.load(handle).get("tool", {}).get("setuptools", {}).get("package-dir", {})
        except (OSError, tomllib.TOMLDecodeError):
            parser = configparser.ConfigParser()
            try:
                parser.read(os.path.join(base, "setup.cfg"))
                lines = parser.get("options", "package_dir", fallback="").splitlines()
            except configparser.Error:
                lines = []
            mapping = dict(line.split("=", 1) for line in lines if "=" in line)
        for package, directory in mapping.items():
            package, directory = package.strip(), directory.strip().strip("/")
            if package and isinstance(directory, str):
                found.append(("/".join(part for part in (folder, directory) if part), package))
    return sorted(found, key=lambda pair: len(pair[0]), reverse=True)


class FileFacts:
    def __init__(self, path):
        self.path = path
        self.test = is_test(path)
        self.declarations = {}  # key -> [start, end] of its first definition
        self.ranges = []  # (key, start, end) of every definition, a name defined twice in one scope included
        self.methods = {}  # class key -> set of method names
        self.attributes = {}  # class key -> names its objects hold (`self.x = ...`, class-level assignments)
        self.bases = {}  # class key -> [binding]
        self.exports = {}  # name -> key (module-level definitions)
        self.reexports = {}  # name -> (module, name) imported names, visible as attributes of this module
        self.references = []  # dicts
        self.entries = {}  # key -> [(line, decorator text)]
        self.star_modules = []  # modules `from x import *` brings names from
        self.instances = {}  # module-level name -> class name it was made with (`registry = ToolRegistry()`)
        self.stamp = None  # the file's (mtime, size) when it was read: a saved fact is reused only while it holds
        self.warnings = []


class Scope:
    def __init__(self, kind, key, bound, declared_global=(), imports=None, constructed=None):
        self.kind, self.key, self.bound, self.globals, self.imports = kind, key, bound, set(declared_global), imports or {}
        self.constructed = constructed or {}  # local name -> the call that made its only value (`cart = Cart()`)


# Builtins that call a method of their first argument.
BUILTIN_DUNDERS = {"len": "__len__", "str": "__str__", "repr": "__repr__", "bool": "__bool__", "iter": "__iter__", "next": "__next__", "hash": "__hash__", "bytes": "__bytes__", "int": "__int__", "float": "__float__", "abs": "__abs__", "reversed": "__reversed__", "format": "__format__"}

# Standard decorators that change how a function is called, not who calls it.
PLAIN_DECORATORS = {"abstractmethod", "classmethod", "staticmethod", "property", "cached_property", "setter", "getter", "deleter", "wraps", "lru_cache", "cache", "dataclass", "overload", "override", "final", "contextmanager", "asynccontextmanager", "total_ordering", "singledispatch", "singledispatchmethod"}


class Reader(ast.NodeVisitor):
    """One file: declarations first (a method may be called above where it is written), then references."""

    def __init__(self, facts, tree, modules):
        self.facts = facts
        self.tree = tree
        self.modules = modules
        self.module_bindings = {}
        self.star_modules = []
        self.package = modules.package_of(facts.path)
        self.scopes = []  # Scope records, outermost first
        self.called = set()  # ids of the nodes a Call calls (its `func`), marked before they are visited
        self.held = {}  # class key -> {attribute: the call that made its only value (`self.repo = Repo()`)}

    def absolute(self, module, level):
        if level == 0:
            return module or ""
        base = self.package.split(".") if self.package else []
        if level > 1:
            base = base[: len(base) - (level - 1)]
        return ".".join([*base, module] if module else base)

    def import_bindings(self, node):
        """(local name, binding) for each name an import statement binds; `*` gives (None, module)."""
        bindings = []
        for alias in node.names:
            if isinstance(node, ast.Import):
                dotted = alias.name
                local = alias.asname or dotted.split(".")[0]
                target = dotted if alias.asname else dotted.split(".")[0]
                module = self.modules.resolve(target, self.facts.path)
                bindings.append((local, ("import", module, "*", target) if module else ("package",)))
                continue
            base = self.absolute(node.module, node.level)
            importer = self.facts.path if node.level == 0 else None
            if alias.name == "*":
                bindings.append((None, self.modules.resolve(base, importer)))
                continue
            local = alias.asname or alias.name
            submodule = self.modules.resolve(f"{base}.{alias.name}" if base else alias.name, importer)
            module = self.modules.resolve(base, importer)
            if submodule:
                bindings.append((local, ("import", submodule, "*", f"{base}.{alias.name}" if base else alias.name)))
            elif module:
                bindings.append((local, ("import", module, alias.name, alias.name)))
            else:
                bindings.append((local, ("package",)))
        return bindings

    def bind_import(self, node):
        for local, binding in self.import_bindings(node):
            if local is None:
                # Every public name of that module: a name bound nowhere else may come from it.
                if binding:
                    self.star_modules.append(binding)
                    self.facts.star_modules.append(binding)
                continue
            self.module_bindings[local] = binding
            if binding[0] == "import":
                self.facts.reexports[local] = (binding[1], binding[2])

    def body_imports(self, node):
        """Imports written in a function's own body (not in functions nested in it)."""
        found, stack = {}, list(node.body)
        while stack:
            child = stack.pop()
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
                continue
            if isinstance(child, (ast.Import, ast.ImportFrom)):
                for local, binding in self.import_bindings(child):
                    if local is not None:
                        found[local] = binding
            stack.extend(ast.iter_child_nodes(child))
        return found

    # declarations
    def declare(self, node, prefix):
        for child in ast.iter_child_nodes(node):
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                key = f"{prefix}.{child.name}" if prefix else child.name
                self.facts.declarations.setdefault(key, [child.lineno, child.end_lineno])
                self.facts.ranges.append((key, child.lineno, child.end_lineno))
                if not prefix:
                    self.facts.exports[child.name] = key
                if isinstance(child, ast.ClassDef):
                    self.facts.methods[key] = {item.name for item in child.body if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))}
                    self.held[key] = self.attribute_constructions(child)
                    held = self.facts.attributes.setdefault(key, set())
                    for item in child.body:
                        targets = item.targets if isinstance(item, ast.Assign) else [item.target] if isinstance(item, ast.AnnAssign) else []
                        held.update(target.id for target in targets if isinstance(target, ast.Name))
                for decorator in getattr(child, "decorator_list", []):
                    called = decorator.func if isinstance(decorator, ast.Call) else decorator
                    name = called.attr if isinstance(called, ast.Attribute) else called.id if isinstance(called, ast.Name) else None
                    if name not in PLAIN_DECORATORS:
                        self.facts.entries.setdefault(key, []).append((decorator.lineno, ast.unparse(decorator)[:80]))
                self.declare(child, key)
            elif isinstance(child, (ast.Import, ast.ImportFrom)) and not prefix:
                self.bind_import(child)
            elif isinstance(child, ast.Assign) and not prefix and len(child.targets) == 1 and isinstance(child.targets[0], ast.Name) and isinstance(child.value, ast.Call) and isinstance(child.value.func, ast.Name):
                self.facts.instances[child.targets[0].id] = child.value.func.id
            elif not isinstance(child, ast.expr):
                self.declare(child, prefix)

    @staticmethod
    def constructions(node):
        """Locals of a function given one value, by a call (`cart = Cart()`, `with Lock() as held`): the call says
        what the local is. A name assigned twice, or also bound another way, could be anything."""
        made, counts, stack = {}, {}, list(node.body)
        while stack:
            child = stack.pop()
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
                continue
            if isinstance(child, ast.Assign) and len(child.targets) == 1 and isinstance(child.targets[0], ast.Name) and isinstance(child.value, ast.Call):
                made[child.targets[0].id] = child.value
            elif isinstance(child, ast.withitem) and isinstance(child.optional_vars, ast.Name) and isinstance(child.context_expr, ast.Call):
                made[child.optional_vars.id] = child.context_expr
            if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Store):
                counts[child.id] = counts.get(child.id, 0) + 1
            stack.extend(ast.iter_child_nodes(child))
        return {name: call for name, call in made.items() if counts.get(name) == 1}

    @staticmethod
    def attribute_constructions(class_node):
        """`self.x = Call(...)` written in the class's methods, for attributes given that one value only."""
        made, counts = {}, {}
        for method in class_node.body:
            if not isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            for child in ast.walk(method):
                targets = child.targets if isinstance(child, ast.Assign) else [child.target] if isinstance(child, (ast.AnnAssign, ast.AugAssign)) else []
                for target in targets:
                    if isinstance(target, ast.Attribute) and isinstance(target.value, ast.Name) and target.value.id == "self":
                        # `self.x = None` until it is made is still one value.
                        value = getattr(child, "value", None)
                        counts[target.attr] = counts.get(target.attr, 0) + (0 if value is None or (isinstance(value, ast.Constant) and value.value is None) else 1)
                        if isinstance(child.value, ast.Call):
                            made[target.attr] = child.value
        return {name: call for name, call in made.items() if counts.get(name) == 1}

    def held_by_self(self, expression):
        """The call that made `self.x`, when `x` is given one value in the class."""
        if isinstance(expression, ast.Attribute) and isinstance(expression.value, ast.Name) and expression.value.id == "self":
            return self.held.get(self.enclosing_class(), {}).get(expression.attr)
        return None

    def constructed(self, name):
        """The call that made a name's only value: a local of the function it is read in or of one around it, or a
        module global (`_lock = threading.Lock()`)."""
        for scope in reversed(self.scopes):
            if scope.kind in ("function", "module") and name in scope.bound:
                return scope.constructed.get(name)
        return None

    # scopes
    def bound_in(self, node):
        """Names a function or module body binds (assignment, def, import, for, with, except, parameters)."""
        bound, declared_global = set(), set()
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            arguments = node.args
            for argument in [*arguments.posonlyargs, *arguments.args, *arguments.kwonlyargs, arguments.vararg, arguments.kwarg]:
                if argument is not None:
                    bound.add(argument.arg)
        body = node.body if isinstance(node.body, list) else [node.body]
        stack = list(body)
        while stack:
            child = stack.pop()
            if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                bound.add(child.name)
                stack.extend(child.decorator_list)
                continue
            if isinstance(child, ast.Lambda):
                continue
            if isinstance(child, (ast.Global, ast.Nonlocal)):
                declared_global.update(child.names)
            elif isinstance(child, ast.Name) and isinstance(child.ctx, (ast.Store, ast.Del)):
                bound.add(child.id)
            elif isinstance(child, (ast.Import, ast.ImportFrom)):
                for alias in child.names:
                    bound.add((alias.asname or alias.name).split(".")[0])
            elif isinstance(child, ast.ExceptHandler) and child.name:
                bound.add(child.name)
            elif isinstance(child, ast.MatchAs) and child.name:
                bound.add(child.name)
            stack.extend(ast.iter_child_nodes(child))
        return bound - declared_global, declared_global

    def current_key(self):
        for scope in reversed(self.scopes):
            if scope.kind in ("function", "class") and scope.key:
                return scope.key
        return "(module)"

    def binding_of(self, name):
        """What a loaded name refers to: a definition here, an import, a local value, or nothing (global)."""
        for index in range(len(self.scopes) - 1, -1, -1):
            scope = self.scopes[index]
            if name in scope.globals:
                break
            if scope.kind == "class" and index != len(self.scopes) - 1:
                continue  # a class body's names are not visible inside its methods
            if name in scope.bound:
                if scope.kind == "module":
                    break
                if name in scope.imports:
                    return scope.imports[name]
                own = f"{scope.key}.{name}" if scope.kind in ("function", "class") and scope.key else None
                return ("own", own) if own in self.facts.declarations else ("local",)
        if name in self.module_bindings:
            return self.module_bindings[name]
        if name in self.facts.declarations:
            return ("own", name)
        if self.scopes and name in self.scopes[0].bound:
            return ("local",)
        return ("builtin",) if name in BUILTINS else ("global",)

    def enclosing_class(self):
        """The class whose method `self` belongs to, through functions and lambdas nested in that method."""
        inside_function = False
        for scope in reversed(self.scopes):
            if scope.kind == "class":
                return scope.key if inside_function else None
            if scope.kind in ("function", "lambda"):
                inside_function = True
        return None

    def visit_Module(self, node):
        self.declare(node, "")
        bound, globals_ = self.bound_in(node)
        self.scopes.append(Scope("module", "(module)", bound, globals_, constructed=self.constructions(node)))
        self.generic_visit(node)
        self.scopes.pop()

    def visit_scope(self, node, kind):
        parent = self.current_key()
        key = node.name if parent == "(module)" else f"{parent}.{node.name}"
        for decorator in node.decorator_list:
            self.visit(decorator)
        if kind == "class":
            self.facts.bases[key] = [self.base_binding(base) for base in node.bases]
            for base in node.bases:
                self.visit(base)  # a subclass depends on its base: listed as a reference
            for keyword in node.keywords:
                self.visit(keyword)
            bound = {item.name for item in node.body if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))}
            self.scopes.append(Scope("class", key, bound))
            for statement in node.body:
                self.visit(statement)
        else:
            for default in [*node.args.defaults, *node.args.kw_defaults]:
                if default is not None:
                    self.visit(default)
            bound, globals_ = self.bound_in(node)
            self.scopes.append(Scope("function", key, bound, globals_, self.body_imports(node), self.constructions(node)))
            for statement in node.body:
                self.visit(statement)
        self.scopes.pop()

    def visit_FunctionDef(self, node):
        self.visit_scope(node, "function")

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node):
        self.visit_scope(node, "class")

    def visit_Lambda(self, node):
        bound, globals_ = self.bound_in(node)
        self.scopes.append(Scope("lambda", None, bound, globals_))
        self.visit(node.body)
        self.scopes.pop()

    def base_binding(self, base):
        """A base class as written: `Base`, or `module.Base` through an imported module."""
        if isinstance(base, ast.Name):
            return self.binding_of(base.id)
        if isinstance(base, ast.Attribute) and isinstance(base.value, ast.Name):
            module = self.binding_of(base.value.id)
            if module[0] == "import" and module[2] == "*":
                return ("import", module[1], base.attr)
        return ("expression",)

    def module_of_chain(self, node):
        """`pkg.core` in `pkg.core.helper()`, when `pkg` is an imported module: the file of `pkg.core`."""
        names = []
        while isinstance(node, ast.Attribute):
            names.append(node.attr)
            node = node.value
        if not isinstance(node, ast.Name) or not names:
            return None
        binding = self.binding_of(node.id)
        if binding[0] != "import" or binding[2] != "*":
            return None
        return self.modules.resolve(".".join([binding[3], *reversed(names)]), self.facts.path)

    def is_call(self, node):
        return id(node) in self.called

    def add(self, **reference):
        reference.setdefault("from", self.current_key())
        self.facts.references.append(reference)

    def visit_Name(self, node):
        if not isinstance(node.ctx, ast.Load):
            return
        binding = self.binding_of(node.id)
        kind = "call" if self.is_call(node) else "reference"
        # `try: from x import f` / `except ImportError: def f(): ...`: either may be the one that runs.
        if binding[0] in ("import", "package") and node.id in self.facts.declarations and len(self.scopes) and not any(node.id in scope.bound for scope in self.scopes[1:]):
            self.add(to="own", key=node.id, line=node.lineno, kind=kind, likely=True)
        if binding[0] == "own":
            self.add(to="own", key=binding[1], line=node.lineno, kind=kind)
        elif binding[0] == "import":
            self.add(to="import", module=binding[1], name=binding[2], member=None, line=node.lineno, kind=kind)
        elif binding[0] == "global" and self.star_modules:
            self.add(to="star", modules=self.star_modules, name=node.id, line=node.lineno, kind=kind)
        elif binding[0] == "global":
            self.add(to="global", name=node.id, line=node.lineno, kind=kind)

    def visit_Attribute(self, node):
        value = node.value
        if not isinstance(node.ctx, ast.Load):
            if isinstance(value, ast.Name) and value.id == "self" and self.enclosing_class():
                self.facts.attributes.setdefault(self.enclosing_class(), set()).add(node.attr)
            return self.visit(value)  # `self.command = x` stores, it does not use
        kind = "call" if self.is_call(node) else "reference"
        if isinstance(value, ast.Call) and isinstance(value.func, ast.Name) and value.func.id == "super":
            # `super(Mixin, self)` starts after the class it names; `super()` after the enclosing one.
            named = self.binding_of(value.args[0].id) if value.args and isinstance(value.args[0], ast.Name) else None
            owner = named[1] if named and named[0] == "own" else self.enclosing_class()
            self.add(to="super", owner=owner, name=node.attr, line=node.lineno, kind=kind)
            return self.visit(value)
        chained = self.module_of_chain(value) if isinstance(value, ast.Attribute) else None
        if chained:
            self.add(to="import", module=chained, name="*", member=node.attr, line=node.lineno, kind=kind)
            return
        if isinstance(value, ast.Name) and isinstance(value.ctx, ast.Load):
            binding = self.binding_of(value.id)
            if binding[0] == "import":
                self.add(to="import", module=binding[1], name=binding[2], member=node.attr, line=node.lineno, kind=kind)
                return
            if binding[0] == "package":
                return
            made = self.constructed(value.id) if binding[0] == "local" else None
            if made is not None and self.member_of_construction(made, node.attr, node.lineno, kind):
                return
            if value.id in ("self", "cls"):
                owner = self.enclosing_class()
                if owner and node.attr in self.facts.methods.get(owner, ()):
                    self.add(to="own", key=f"{owner}.{node.attr}", line=node.lineno, kind=kind)
                    return
                self.add(to="self", owner=owner, name=node.attr, line=node.lineno, kind=kind)
                return
        held = self.held_by_self(value)
        if held is not None and self.member_of_construction(held, node.attr, node.lineno, kind):
            return self.visit(value)
        self.add(to="member", name=node.attr, line=node.lineno, kind=kind)
        self.visit(value)

    def implicit(self, expression, members, line, unknown):
        """Methods Python calls without their name being written: `with Lock():` runs `Lock.__enter__`, `for x in
        Bag():` runs `Bag.__iter__`. A constructed class of this repository is certain; an object of unknown
        class (`with self.lock:`) is listed by the method's name when `unknown` allows it."""
        if isinstance(expression, ast.Name) and self.constructed(expression.id) is not None:
            expression = self.constructed(expression.id)
        elif self.held_by_self(expression) is not None:
            expression = self.held_by_self(expression)
        if isinstance(expression, ast.Call):
            for member in members:
                self.member_of_construction(expression, member, line, "call", implicit=True)
        elif unknown and isinstance(expression, (ast.Name, ast.Attribute)):
            for member in members:
                self.add(to="member", name=member, line=line, kind="call", implicit=True)

    def member_of_construction(self, call, member, line, kind, implicit=False):
        """`member` of what `call` makes, when it makes an instance of a class of this repository; returns
        whether it did."""
        function = call.func
        if isinstance(function, ast.Name):
            binding = self.binding_of(function.id)
            if function.id == "cls" and self.enclosing_class():
                binding = ("own", self.enclosing_class())  # `cls()` in a classmethod makes its own class
            if binding[0] == "own" and binding[1] in self.facts.methods:
                owner = binding[1]
                if member in self.facts.methods[owner]:
                    self.add(to="own", key=f"{owner}.{member}", line=line, kind=kind)
                else:
                    self.add(to="self", owner=owner, name=member, line=line, kind=kind)  # inherited: found through the bases
                return True
            if binding[0] == "import" and binding[2] != "*":
                self.add(to="import", module=binding[1], name=binding[2], member=member, line=line, kind=kind, **({"implicit": True} if implicit else {}))
                return True
        elif isinstance(function, ast.Attribute):
            module = self.module_of_chain(function.value) if isinstance(function.value, ast.Attribute) else None
            binding = self.binding_of(function.value.id) if isinstance(function.value, ast.Name) else None
            if binding and binding[0] == "import" and binding[2] == "*":
                module = binding[1]
            if module:
                self.add(to="import", module=module, name=function.attr, member=member, line=line, kind=kind)
                return True
        return False

    def visit_With(self, node):
        for item in node.items:
            self.implicit(item.context_expr, ("__enter__", "__exit__"), node.lineno, unknown=True)
        self.generic_visit(node)

    def visit_AsyncWith(self, node):
        for item in node.items:
            self.implicit(item.context_expr, ("__aenter__", "__aexit__"), node.lineno, unknown=True)
        self.generic_visit(node)

    def visit_For(self, node):
        # A loop over a name is far too common to list for every `__iter__`: only constructed objects count.
        self.implicit(node.iter, ("__aiter__",) if isinstance(node, ast.AsyncFor) else ("__iter__",), node.lineno, unknown=False)
        self.generic_visit(node)

    visit_AsyncFor = visit_For

    def visit_Call(self, node):
        function, arguments = node.func, node.args
        self.called.add(id(function))
        # `len(cart)` runs `cart.__len__`: builtins that call a method of their argument.
        if isinstance(function, ast.Name) and function.id in BUILTIN_DUNDERS and arguments and self.binding_of(function.id)[0] == "builtin":
            self.implicit(arguments[0], (BUILTIN_DUNDERS[function.id],), node.lineno, unknown=False)
        # `make()(...)`, `handler(...)` with `handler` a local value: the object called may define `__call__`.
        if isinstance(function, ast.Call):
            self.implicit(function, ("__call__",), node.lineno, unknown=False)
        elif isinstance(function, ast.Name) and function.id != "cls" and self.binding_of(function.id)[0] == "local":
            self.implicit(function, ("__call__",), node.lineno, unknown=True)
        elif self.held_by_self(function) is not None:
            self.implicit(function, ("__call__",), node.lineno, unknown=False)
        if isinstance(function, ast.Name) and function.id == "getattr" and len(arguments) >= 2 and isinstance(arguments[1], ast.Constant) and isinstance(arguments[1].value, str):
            self.add(to="member", name=arguments[1].value, line=node.lineno, kind="call" if self.is_call(node) else "reference")
        self.generic_visit(node)

    def visit_If(self, node):
        test = node.test
        if isinstance(test, ast.Compare) and isinstance(test.left, ast.Name) and test.left.id == "__name__" and not self.scopes[1:]:
            self.facts.entries.setdefault("(module)", []).append((node.lineno, "__main__"))
        self.generic_visit(node)


SLOT_WAIT_SECONDS = 10


def take_work_slot():
    """One of the machine-wide work slots (`scout hold-slot`), so repositories reading their code at once share the
    machine: the helper process while it is held (close its stdin to free the slot), None when the core hands out
    none. False after SLOT_WAIT_SECONDS without one: a question may be waiting on this work, and the files are then
    read in the driver itself."""
    helper = os.environ.get("SCOUT_SLOT_HELPER")
    if not helper:
        return None
    process = subprocess.Popen([helper, "hold-slot"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    ready, _, _ = select.select([process.stdout], [], [], SLOT_WAIT_SECONDS)
    if ready and process.stdout.readline():
        return process
    process.kill()
    process.wait()
    return False


def read_files(paths, known):
    """Warm-up worker, at the lowest priority: the facts of these files, imports resolved against `known`."""
    try:
        os.nice(19)
    except (AttributeError, OSError):
        pass  # lowering priority is a courtesy some systems refuse
    slot = take_work_slot()
    if slot is False:
        return []
    try:
        modules = Modules(known)
        return [(path, Store.parse_file(path, modules)) for path in paths]
    finally:
        if slot:
            slot.stdin.close()
            slot.wait()


class Store:
    # Below this many files reading on demand is quick enough; above, several processes read them at start.
    WARM_UP_FROM = 300

    def __init__(self):
        self.known = list_files()
        self.modules = Modules(self.known)
        self.warming = []  # futures of [(path, facts)]
        self.respelled = set()  # files whose imports a file appearing or vanishing during the warm-up may change
        self.facts = {}
        self.by_name = {}  # name -> paths with global/member/self references
        self.by_module = {}  # module path -> paths importing it
        self.children = {}  # (path, class key) -> {(path, class key)} naming it as a base
        self.answers = {}  # symbol -> callers, until any file changes
        self.unsaved = False  # facts read since the saved copy was written
        self.saved_once = False
        self.load_saved()

    @staticmethod
    def parse_file(path, modules):
        facts = FileFacts(path)
        read_stamp = stamp(path)
        try:
            with open(os.path.join(REPO, path), encoding="utf-8", errors="replace") as handle:
                tree = ast.parse(handle.read(), filename=path)
            Reader(facts, tree, modules).visit(tree)
        except SyntaxError as error:
            facts = FileFacts(path)
            facts.warnings.append(f"{path}:{error.lineno} could not be parsed ({error.msg}); calls inside it are unknown")
        facts.stamp = read_stamp
        return facts

    def read(self, path):
        self.forget(path)
        if path not in self.known:
            return None
        self.unsaved = True
        return self.install(path, self.parse_file(path, self.modules))

    def install(self, path, facts):
        self.facts[path] = facts
        for reference in facts.references:
            if reference["to"] in ("global", "member", "self", "star", "super"):
                self.by_name.setdefault(reference["name"], set()).add(path)
            elif reference["to"] == "import":
                self.by_module.setdefault(reference["module"], set()).add(path)
                if reference.get("member"):
                    self.by_name.setdefault(reference["member"], set()).add(path)
        for module, _ in facts.reexports.values():
            self.by_module.setdefault(module, set()).add(path)
        for key, bases in facts.bases.items():
            for base in bases:
                named = self.base_target(path, base)
                if named:
                    self.children.setdefault(named, set()).add((path, key))
        return facts

    @staticmethod
    def base_target(path, base):
        """The (path, class key) a base binding names as written: in the same file, or imported."""
        if base[0] == "own":
            return path, base[1]
        if base[0] == "import" and base[2] != "*":
            return base[1], base[2]
        return None

    def forget(self, path):
        facts = self.facts.pop(path, None)
        if not facts:
            return
        for paths in [*self.by_name.values(), *self.by_module.values()]:
            paths.discard(path)
        for key, bases in facts.bases.items():
            for base in bases:
                self.children.get(self.base_target(path, base), set()).discard((path, key))

    def get(self, path):
        return self.facts.get(path) or (self.read(path) if path in self.known else None)

    # Facts saved between sessions: a new server reads them back in under a second instead of parsing every
    # file again (13 s for 8,400 files). A file whose (mtime, size) changed is read again; a file that appeared or
    # vanished makes the files naming its module read again, as `sync` does.
    def saved_path(self):
        cache = os.environ.get("SCOUT_CACHE") or os.path.join(os.path.expanduser("~"), ".cache", "scout")
        return os.path.join(cache, hashlib.sha1(REPO.encode()).hexdigest()[:16], "python-facts")

    def saved_key(self):
        """What every saved fact depends on besides its own file: this driver, the Python reading, the project files."""
        markers = sorted((folder, marker, stamp(os.path.join(folder, marker))) for folder in {folder for path in self.known for folder in project_folders(path)} for marker in PROJECT_MARKERS if stamp(os.path.join(folder, marker)))
        with open(__file__, "rb") as handle:
            driver = hashlib.sha1(handle.read()).hexdigest()
        return (driver, sys.version, markers)

    def load_saved(self):
        try:
            with open(self.saved_path(), "rb") as handle:
                key, known, facts = pickle.loads(zlib.decompress(handle.read()))
        except Exception:  # none yet, or unreadable: read the files
            return
        if key != self.saved_key():
            return
        for path, saved in facts.items():
            if path in self.known and saved.stamp is not None and saved.stamp == stamp(path):
                self.install(path, saved)
        reshaped = known ^ self.known
        for path in self.spelling([self.module_word(path) for path in reshaped]) if reshaped else []:
            self.forget(path)
        self.unsaved = len(self.facts) != len(self.known)
        self.saved_once = not self.unsaved

    def save(self):
        if not self.unsaved or len(self.facts) != len(self.known):
            return
        path = self.saved_path()
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path + ".tmp", "wb") as handle:
                handle.write(zlib.compress(pickle.dumps((self.saved_key(), set(self.known), self.facts), protocol=5), 1))
            os.replace(path + ".tmp", path)
            self.unsaved = False
            self.saved_once = True
        except OSError:
            pass  # a full disk only costs the next server a slower start

    def start_warm_up(self):
        paths = sorted(self.known - self.facts.keys())
        if len(paths) < self.WARM_UP_FROM:
            return
        workers = max(1, min(4, (os.cpu_count() or 2) // 2))
        self.pool = ProcessPoolExecutor(workers)
        size = max(50, len(paths) // (workers * 8))
        self.warming = [self.pool.submit(read_files, paths[start : start + size], self.known) for start in range(0, len(paths), size)]

    def finish_warm_up(self):
        """Takes what the warm-up read, except a file read here since (it changed) or one that names a module that
        appeared or vanished meanwhile (its imports may mean another file now)."""
        warming, self.warming = self.warming, []
        respelled, self.respelled = self.respelled, set()
        for future in warming:
            try:
                results = future.result()
            except Exception:  # a worker that died leaves its files to be read here
                continue
            for path, facts in results:
                if path in self.known and path not in self.facts and path not in respelled:
                    self.install(path, facts)
                    self.unsaved = True
        if warming:
            self.pool.shutdown(wait=False)

    def read_all(self):
        self.finish_warm_up()
        for path in sorted(self.known):
            if path not in self.facts:
                self.read(path)
        # Once every file is read for the first time; edits after that are saved when the server leaves, so a
        # question right after an edit does not wait for a write.
        if not self.saved_once:
            self.save()

    def sync(self, changed):
        reshaped = []
        changed = [path for path in changed if path.endswith(".py")]
        if changed:
            self.answers.clear()
        for path in changed:
            exists = os.path.isfile(os.path.join(REPO, path))
            if exists != (path in self.known):
                reshaped.append(path)
                (self.known.add if exists else self.known.discard)(path)
            self.read(path) if exists else self.forget(path)
        if reshaped:
            # Which file a module name means depends on which files exist: the files that spell the name of an
            # added or removed module (or of its package) may now import something else, and are read again.
            self.modules = Modules(self.known)
            spelled = self.spelling([self.module_word(path) for path in reshaped])
            if self.warming:
                self.respelled.update(spelled)
            for path in spelled:
                if path in self.facts:
                    self.read(path)
        return {"warnings": [warning for facts in self.facts.values() for warning in facts.warnings]}

    @staticmethod
    def module_word(path):
        stem = path[:-3]
        return os.path.basename(os.path.dirname(stem) if stem.endswith("/__init__") or stem == "__init__" else stem)

    def spelling(self, words):
        """Python files whose text has one of these words."""
        words = sorted({word for word in words if word})
        if not words:
            return []
        command = ["git", "grep", "-l", "-w", "-F", "--untracked", *[part for word in words for part in ("-e", word)], "--", "*.py"]
        output = subprocess.run(command, cwd=REPO, capture_output=True, text=True).stdout
        return [path for path in output.splitlines() if path]

    def ref(self, path, key, line=None):
        facts = self.get(path)
        declared = facts.declarations.get(key) if facts else None
        result = {"symbol": f"{path}#{key}", "file": path, "line": line or (declared[0] if declared else 1)}
        if facts and facts.test:
            result["kind"] = "test"
        return result

    def parse(self, symbol):
        path, _, key = symbol.rpartition("#")
        return (path, key) if path in self.known else None

    def aliases(self, path, key):
        """(module, name, member) under which the symbol is reachable from other files, re-exports followed."""
        owner, _, member = key.rpartition(".")
        # `module.helper` for a module-level name; a method only as `Class.method` once the class is imported
        # (`module.Class.method` is two members deep and is matched by name).
        if "." not in key:
            found = [(path, "*", key), (path, key, None)]
        elif owner and "." not in owner:
            found = [(path, owner, member)]
        else:
            found = []
        index = 0
        while index < len(found):
            module, name, item = found[index]
            index += 1
            for other in self.by_module.get(module, ()):
                for local, (source, original) in self.get(other).reexports.items():
                    if source == module and original == name and (other, local, item) not in found:
                        found.append((other, local, item))
        return found

    def resolve_export(self, module, name, seen=None):
        """The (file, key) a module attribute names, re-exports followed."""
        seen = seen or set()
        if (module, name) in seen:
            return None
        seen.add((module, name))
        facts = self.get(module)
        if not facts:
            return None
        if name in facts.exports:
            return module, facts.exports[name]
        if name in facts.reexports:
            return self.resolve_export(*facts.reexports[name], seen)
        for star in facts.star_modules:
            found = self.resolve_export(star, name, seen)
            if found:
                return found
        return None

    def direct_bases(self, path, class_key):
        found = []
        for base in self.get(path).bases.get(class_key, []):
            named = (path, base[1]) if base[0] == "own" else self.resolve_export(base[1], base[2]) if base[0] == "import" and base[2] != "*" else None
            if named and self.get(named[0]) and named[1] in self.get(named[0]).methods and named not in found:
                found.append(named)
        return found

    def linearize(self, path, class_key, visiting=()):
        """The class and its bases in the order Python looks a member up (C3); a cycle or an order Python would
        refuse falls back to depth-first, left to right."""
        here = (path, class_key)
        if here in visiting:
            return [here]
        bases = self.direct_bases(path, class_key)
        sequences = [self.linearize(*base, (*visiting, here)) for base in bases] + [list(bases)]
        result = [here]
        while any(sequences):
            sequences = [sequence for sequence in sequences if sequence]
            head = next((sequence[0] for sequence in sequences if not any(sequence[0] in other[1:] for other in sequences)), None)
            if head is None:
                for sequence in sequences:
                    result.extend(item for item in sequence if item not in result)
                break
            result.append(head)
            sequences = [sequence[1:] if sequence[0] == head else sequence for sequence in sequences]
        return result

    def ancestors_of(self, path, class_key):
        """Base classes, through each other, that this repository defines, nearest first (Python's lookup order)."""
        return self.linearize(path, class_key)[1:]

    def next_definer(self, instance, after, member):
        """The class whose `member` `super()` written in class `after` runs, for an object of class `instance`."""
        order = self.linearize(*instance)
        if after not in order:
            return None
        return next((base for base in order[order.index(after) + 1 :] if member in self.get(base[0]).methods.get(base[1], ())), None)

    def holds(self, path, class_key, name):
        """Whether objects of the class have `name` without `__getattr__`: a method or an assigned attribute of it or
        of a base."""
        for base_path, base_key in self.linearize(path, class_key):
            facts = self.get(base_path)
            if name in facts.methods.get(base_key, ()) or name in facts.attributes.get(base_key, ()):
                return True
        return False

    def first_definer(self, path, class_key, member):
        """The class whose `member` an instance of this class runs: itself, else its nearest base defining it."""
        if member in self.get(path).methods.get(class_key, ()):
            return path, class_key
        for base in self.ancestors_of(path, class_key):
            if member in self.get(base[0]).methods.get(base[1], ()):
                return base
        return None

    def related(self, file, class_key, path, owner):
        """Whether two classes are the same or one derives from the other (their `self.` calls are judged above)."""
        if not class_key:
            return False
        if (file, class_key) == (path, owner):
            return True
        return (path, owner) in self.ancestors_of(file, class_key) or (file, class_key) in self.ancestors_of(path, owner)

    def subclasses_of(self, path, class_key):
        """Classes that name this one as a base, directly or through each other."""
        result, pending = [], [(path, class_key)]
        while pending:
            for child in sorted(self.children.get(pending.pop(), ())):
                if child not in result:
                    result.append(child)
                    pending.append(child)
        return result

    def callers(self, symbol):
        if symbol not in self.answers:
            self.answers[symbol] = self.compute_callers(symbol)
        return self.answers[symbol]

    def compute_callers(self, symbol):
        target = self.parse(symbol)
        if not target:
            return None
        self.read_all()
        path, key = target
        facts = self.get(path)
        if not facts:
            return []
        edges = []

        def edge(file, reference, confidence):
            edges.append({"from": self.ref(file, reference["from"]), "at": {"file": file, "line": reference["line"]}, "kind": reference["kind"], "confidence": confidence})

        for line, decorator in facts.entries.get(key, []):
            edges.append({"from": {"symbol": f"@{decorator} ({path}:{line})", "kind": "entry"}, "at": {"file": path, "line": line}, "kind": "decorator", "confidence": "exact"})
        for reference in facts.references:
            if reference["to"] == "own" and reference["key"] == key:
                edge(path, reference, "likely" if reference.get("likely") else "exact")
        owner, _, member = key.rpartition(".")
        if owner in facts.methods:
            # `self.area()` in a subclass runs this method when no class between them defines its own.
            for sub_path, sub_key in self.subclasses_of(path, owner):
                if self.first_definer(sub_path, sub_key, member) != (path, owner):
                    continue
                for reference in self.get(sub_path).references:
                    if reference["to"] == "self" and reference["owner"] == sub_key and reference["name"] == member:
                        edge(sub_path, reference, "exact")
            # Written in a base class, `self.area()` runs this override when the object is of this class.
            for base_path, base_key in self.ancestors_of(path, owner):
                for reference in self.get(base_path).references:
                    own = reference["to"] == "own" and reference["key"] == f"{base_key}.{member}"
                    hook = reference["to"] == "self" and reference["owner"] == base_key and reference["name"] == member
                    if own or hook:
                        edge(base_path, reference, "likely")
            # `super().area()` in a subclass runs the nearest base class's own definition.
            for file in self.by_name.get(member, ()):
                for reference in self.get(file).references:
                    if reference["to"] == "super" and reference["name"] == member and reference["owner"]:
                        caller = (file, reference["owner"])
                        if self.next_definer(caller, caller, member) == (path, owner):
                            edge(file, reference, "exact")
                        # Cooperative inheritance: in an object of a subclass with several bases, the method after
                        # the caller's class in that subclass's order may be a sibling's (`Diamond(Left, Right)`).
                        elif any(self.next_definer(sub, caller, member) == (path, owner) for sub in self.subclasses_of(*caller)):
                            edge(file, reference, "likely")
            # A class is constructed by calling it: `Cls(...)` runs `Cls.__init__` (a dataclass's then its
            # `__post_init__`), and so does a subclass's construction when it does not define its own.
            if member in ("__init__", "__new__", "__post_init__"):
                for constructed_path, constructed in [(path, owner), *self.subclasses_of(path, owner)]:
                    if self.first_definer(constructed_path, constructed, member) != (path, owner):
                        continue
                    for found in self.callers(f"{constructed_path}#{constructed}") or []:
                        if found["kind"] == "call":
                            edges.append(found)
        # `__getattr__` runs when an object lacks the attribute read: `self.x` or `cart.x` naming nothing the class
        # (or a base) defines or assigns.
        if member == "__getattr__" and owner in facts.methods:
            family = {(path, owner), *self.subclasses_of(path, owner)}
            for file in list(self.facts):
                for reference in self.get(file).references:
                    if reference["to"] == "self" and reference["owner"] and (file, reference["owner"]) in family and not self.holds(file, reference["owner"], reference["name"]):
                        edge(file, reference, "likely")
            # Through the class imported elsewhere: `cart = Cart(); cart.item_0`.
            for class_path, class_key in family:
                for module, name, item in self.aliases(class_path, class_key):
                    for importer in self.by_module.get(module, ()) if item is None else ():
                        for reference in self.get(importer).references:
                            member_read = reference["to"] == "import" and reference["module"] == module and reference["name"] == name and reference.get("member")
                            if member_read and not self.holds(class_path, class_key, reference["member"]):
                                edge(importer, reference, "likely")
        # Reached by the name of a subclass that inherits it unchanged too: `Cart().add()` runs `Base.add`.
        inheriting = [(sub_path, f"{sub_key}.{member}") for sub_path, sub_key in self.subclasses_of(path, owner) if self.first_definer(sub_path, sub_key, member) == (path, owner)] if owner in facts.methods else []
        for module, name, item in [alias for named in [(path, key), *inheriting] for alias in self.aliases(*named)]:
            for importer in self.by_module.get(module, ()):
                for reference in self.get(importer).references:
                    if reference["to"] != "import" or reference["module"] != module:
                        continue
                    if name == "*":
                        matches = reference["name"] == "*" and reference["member"] == (item.split(".")[0] if item else None)
                    else:
                        matches = reference["name"] == name and (item is None or reference["member"] == item)
                    if matches and item is None and reference.get("member"):
                        # `for x in make():` also runs `__iter__` of the result; the call itself is listed already.
                        if not reference.get("implicit"):
                            edge(importer, {**reference, "kind": "reference"}, "exact")
                    elif matches:
                        edge(importer, reference, "exact")
        # Names an untyped object or an unbound name could mean this symbol by.
        name = key.rpartition(".")[2]
        is_method = owner in facts.methods
        family = {(path, owner), *self.subclasses_of(path, owner)} if is_method else set()
        # Files that import the class itself (or are its own): where an object of unknown class may be one of it.
        naming_class = {path} | {
            importer
            for module, alias, item in (self.aliases(path, owner) if is_method else [])
            if item is None
            for importer in self.by_module.get(module, ())
            if any(reference["to"] == "import" and reference["module"] == module and reference["name"] == alias for reference in self.get(importer).references)
        }
        for file in self.by_name.get(name, ()):
            for reference in self.get(file).references:
                if is_method and reference["to"] == "import" and reference.get("member") == name and reference["name"] != "*":
                    confidence = self.member_of_import(reference["module"], reference["name"], family)
                    # `with make_scope():` runs `__enter__` of whatever the function returns: as for any object of
                    # unknown class, only where the class is imported.
                    if confidence == "unresolved" and name.startswith("__") and file not in naming_class:
                        continue
                    if confidence:
                        edge(file, reference, confidence)
                    continue
                if reference.get("name") != name:
                    continue
                if reference["to"] == "member" and is_method:
                    # `with lock:` or `handler()` on an object of unknown class: thousands of them in a large
                    # project, so only where the class is imported (or in its own file).
                    if not reference.get("implicit") or file in naming_class:
                        edge(file, reference, "unresolved")
                elif reference["to"] == "self" and "." in key and reference["owner"] != owner and not self.related(file, reference["owner"], path, owner):
                    # The calling class's own hierarchy defines it elsewhere: not this method.
                    if not (reference["owner"] and self.first_definer(file, reference["owner"], name)):
                        edge(file, reference, "unresolved")
                elif reference["to"] == "global" and "." not in key:
                    edge(file, reference, "unresolved")
                elif reference["to"] == "star" and "." not in key:
                    # `from lib import *`: exact when the starred module defines (or re-exports) this very name.
                    defined = [self.resolve_export(module, name) for module in reference["modules"]]
                    if (path, key) in defined:
                        edge(file, reference, "exact")
                    elif not any(defined):
                        edge(file, reference, "unresolved")
        return edges

    def member_of_import(self, module, imported, family, depth=0):
        """How surely `imported.member(...)` runs a method of a class in `family`: `likely` for a module-level
        instance of it, `unresolved` for any other object (a function's result, an instance of unknown class),
        None for a class or an instance of another class, whose members are matched through their aliases."""
        facts = self.get(module)
        if facts is None or depth > 8:
            return "unresolved"
        made = facts.instances.get(imported)
        if made is not None:
            made_from = (module, made) if made in facts.methods else self.resolve_export(*facts.reexports[made]) if made in facts.reexports else None
            if made_from is None:
                return "unresolved"
            return "likely" if made_from in family else None
        if imported in facts.reexports:
            return self.member_of_import(*facts.reexports[imported], family, depth + 1)
        key = facts.exports.get(imported)
        return None if key in facts.methods else "unresolved"

    def resolve(self, query):
        if "::" in query or "\\" in query:
            return []
        direct = self.parse(query)
        if direct:
            facts = self.get(direct[0])
            return [self.ref(*direct)] if facts and (direct[1] in facts.declarations or direct[1] == "(module)") else []
        self.read_all()
        return [self.ref(path, query) for path, facts in sorted(self.facts.items()) if query in facts.declarations]

    def locate(self, path, line):
        if path not in self.known:
            return None
        facts = self.get(path)
        best = None
        for key, start, end in facts.ranges:
            if start <= line <= end and (best is None or start >= best[1]):
                best = (key, start)
        return self.ref(path, best[0], best[1]) if best else self.ref(path, "(module)", 1)


def main():
    store = Store()
    store.start_warm_up()
    handlers = {
        "probe": lambda request: {"driver": "python", "python": sys.version.split()[0], "files": len(store.known), "filesRead": len(store.facts)},
        "sync": lambda request: store.sync(request.get("changed", [])),
        "resolve": lambda request: store.resolve(request.get("query", "")),
        "locate": lambda request: store.locate(request.get("file", ""), request.get("line", 1)),
        "callers": lambda request: store.callers(request.get("symbol", "")),
    }
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        try:
            handler = handlers.get(request.get("op"))
            response = {"id": request.get("id"), "result": handler(request) if handler else None}
        except Exception as error:  # an answer with the reason beats a silent driver
            response = {"id": request.get("id"), "error": f"{type(error).__name__}: {error}"}
        sys.stdout.write(json.dumps(response) + "\n")
        sys.stdout.flush()
    store.save()  # the server is leaving: what changed since is saved for the next one


if __name__ == "__main__":
    main()
