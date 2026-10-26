// JavaScript / TypeScript / Vue driver. It reads code with the repository's own `typescript` parser and Vue's own
// `vue/compiler-sfc`, and resolves module paths with the repository's own bundler configuration (Vite, else
// tsconfig/jsconfig paths). There is no whole-program type checker: for a large app that costs gigabytes and
// seconds. What each file declares, imports, exports and references is kept per file and re-read when it changes.
//
// Symbols are `path#key`: `formatDate`, `OrderService.place` (class method), `useCart.load` (function declared
// inside another), `default` (a Vue component or default export), `(module)` (top-level code of a file).
// A reference is `exact` when the name is bound for certain: an import of the target's module (re-exports
// followed), a binding in the same file, `this.method()` in the same class or component, a template expression
// or tag of the component. A member reached through an object of unknown origin (`store.load()`) or a name bound
// nowhere (a global, a mixin) is listed as `unresolved`, never dropped.

import { createRequire } from 'node:module';
import { execFileSync, spawn } from 'node:child_process';
import { readFileSync, existsSync, appendFileSync, openSync, readSync, closeSync, statSync, mkdirSync, writeFileSync, renameSync } from 'node:fs';
import { availableParallelism, setPriority, homedir } from 'node:os';
import { createHash } from 'node:crypto';
import { gzipSync, gunzipSync } from 'node:zlib';
import { join, dirname, resolve as resolvePath, relative, extname, basename, sep } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';

const mode = process.argv[2]?.startsWith('--') ? process.argv[2] : null;
if (mode === '--bundler-config') {
  await printBundlerConfig(resolvePath(process.argv[3]));
  process.exit(0);
}

const driverPath = fileURLToPath(import.meta.url);
const repoRoot = resolvePath(process.argv[mode ? 3 : 2]);
const requireFromRepo = createRequire(join(repoRoot, 'package.json'));
/** The parsers installed with this tool (`scout setup`), for a repository that has none of its own. */
const requireBundled = createRequire(join(dirname(fileURLToPath(import.meta.url)), 'package.json'));
const ts = loadFromRepo('typescript');
const vue = loadFromRepo('vue/compiler-sfc');
const svelte = loadFromRepo('svelte/compiler');
const SCRIPT_EXTENSIONS = new Set(['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs', '.vue', '.svelte', '.mdx']);
const VITE_DEFAULT_EXTENSIONS = ['.mjs', '.js', '.mts', '.ts', '.jsx', '.tsx', '.json'];
const TEST_PATH = /(^|\/)(tests?|__tests__|e2e)\/|\.(test|spec)\.[cm]?[jt]sx?$/;
/** Objects of the JavaScript runtime and the browser: their members are never this repository's functions. */
const RUNTIME_GLOBALS = new Set(['Promise', 'Object', 'Array', 'JSON', 'Math', 'Number', 'String', 'Boolean', 'Reflect', 'Symbol', 'Date', 'Intl', 'RegExp', 'Error', 'Map', 'Set', 'WeakMap', 'WeakSet', 'URL', 'URLSearchParams', 'console', 'window', 'document', 'navigator', 'globalThis', 'process', 'Buffer', 'crypto', 'location', 'history', 'localStorage', 'sessionStorage', 'performance', 'Notification', 'caches', 'self']);
const NO_TYPESCRIPT = 'typescript is installed neither in this repository nor with scout (run `scout setup`); callers in JavaScript/TypeScript/Vue files are unknown';

/** Shared by the driver and its warm-up readers: the bundler's module resolution and the set of script files. */
let resolution;
let known;

/** Appends one timing line to the file named by SCOUT_PROFILE, when set; used to measure, never to answer. */
function profilePhase(phase, startedAt, details = {}) {
  if (!process.env.SCOUT_PROFILE) return;
  appendFileSync(process.env.SCOUT_PROFILE, `. ${phase.padEnd(28)} ${(performance.now() - startedAt).toFixed(1).padStart(7)} ms ${JSON.stringify(details)}\n`);
}

/** The repository's own copy first (the version its code is written for), else the one installed with this tool. */
function loadFromRepo(name) {
  for (const require of [requireFromRepo, requireBundled]) {
    try {
      return require(name);
    } catch {
      // not installed there
    }
  }
  return null;
}

/** ESM entry of a package installed in the repository (a bare `import()` would resolve from this driver's folder). */
function importFromRepo(require, name) {
  let directory = dirname(require.resolve(name));
  while (!existsSync(join(directory, 'package.json')) || JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8')).name !== name) {
    const parent = dirname(directory);
    if (parent === directory) throw new Error(`${name}: package.json not found`);
    directory = parent;
  }
  const manifest = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
  let entry = manifest.exports?.['.'] ?? manifest.exports ?? manifest.module ?? manifest.main ?? 'index.js';
  while (entry && typeof entry === 'object') entry = entry.import ?? entry.default ?? entry.node;
  return import(pathToFileURL(join(directory, entry)).href);
}

/**
 * Runs in a short-lived child: loading a bundler config executes it and pulls in its plugins (about 140 MB),
 * which the long-lived driver should not keep.
 */
async function printBundlerConfig(root) {
  const require = createRequire(join(root, 'package.json'));
  const vite = await importFromRepo(require, 'vite');
  const loaded = await vite.loadConfigFromFile({ command: 'serve', mode: 'development' }, undefined, root, 'silent');
  const alias = loaded?.config?.resolve?.alias ?? {};
  const entries = Array.isArray(alias) ? alias : Object.entries(alias).map(([find, replacement]) => ({ find, replacement }));
  const config = JSON.stringify({
      bundler: `vite ${vite.version}`,
      aliases: entries.map(({ find, replacement }) => (find instanceof RegExp ? { pattern: find.source, flags: find.flags.replace('g', ''), replacement } : { find, replacement })),
      extensions: loaded?.config?.resolve?.extensions ?? null,
      inputs: (loaded?.dependencies ?? []).map((path) => relative(root, resolvePath(root, path)).split(sep).join('/')),
      ...(await resolvedViteFacts(vite, root)),
    });
  // A pipe takes large writes asynchronously: exiting before the callback would cut the output.
  await new Promise((resolve) => process.stdout.write(config, resolve));
}

/**
 * Components a bundler plugin imports by name (unplugin-vue-components and the like expose `api.findComponent`).
 * The plugin decides; this only offers it the names the repository's `.vue` files could go by.
 */
async function resolvedViteFacts(vite, root) {
  const resolved = await vite.resolveConfig({ root, logLevel: 'silent' }, 'serve', 'development');
  const publicDir = resolved.publicDir ? relative(root, resolved.publicDir).split(sep).join('/') : null;
  const input = resolved.build?.rollupOptions?.input ?? [];
  const entries = (typeof input === 'string' ? [input] : Array.isArray(input) ? input : Object.values(input)).map((path) => relative(root, resolvePath(root, path)).split(sep).join('/'));
  return { publicDir: publicDir && !publicDir.startsWith('..') ? publicDir : null, entries, autoComponents: await autoImportedComponents(resolved, root) };
}

async function autoImportedComponents(resolved, root) {
  const plugin = resolved.plugins.find((candidate) => typeof candidate.api?.findComponent === 'function');
  if (!plugin) return {};
  // The plugin scans its component folders on the first file it transforms.
  const transform = typeof plugin.transform === 'function' ? plugin.transform : plugin.transform?.handler;
  await transform?.call({ error: (error) => { throw error; }, warn: () => {} }, '_resolveComponent("ScoutProbe")', join(root, 'scout-probe.vue'));
  const components = {};
  const vueFiles = execFileSync('git', ['ls-files', '-co', '--exclude-standard', '-z', '--', '*.vue'], { cwd: root, encoding: 'utf8', maxBuffer: 1 << 28 }).split('\0').filter(Boolean);
  for (const file of vueFiles) {
    for (const name of componentNameCandidates(file)) {
      if (name in components) continue;
      const info = await plugin.api.findComponent(name);
      const from = typeof info?.from === 'string' ? info.from.split('?')[0] : null;
      if (!from) continue;
      const path = from.startsWith(root) ? relative(root, from).split(sep).join('/') : from.startsWith('/') ? from.slice(1) : null;
      if (path && existsSync(join(root, path))) components[name] = path;
    }
  }
  return components;
}

function pascalCase(text) {
  return text.replace(/(^|[-_\s/.])(\w)/g, (_, __, letter) => letter.toUpperCase()).replace(/[^A-Za-z0-9]/g, '');
}

/** `a/b/user-card.vue` may be `UserCard`, `BUserCard` or `ABUserCard`, depending on how the plugin names folders. */
function componentNameCandidates(path) {
  const parts = path.replace(/\.vue$/, '').split('/');
  const names = [];
  for (let count = 1; count <= parts.length; count++) names.push(pascalCase(parts.slice(-count).join('-')));
  if (parts.at(-1) === 'index' && parts.length > 1) names.push(pascalCase(parts.at(-2)));
  return names;
}

/** How bare specifiers map to files: the bundler's own answer when there is one, else tsconfig/jsconfig `paths`. */
function readModuleResolution() {
  const warnings = [];
  const packages = workspacePackages();
  const packageFiles = packages.map((entry) => `${entry.dir ? `${entry.dir}/` : ''}package.json`);
  const configFiles = ['tsconfig.json', 'jsconfig.json'].filter((name) => existsSync(join(repoRoot, name)));
  const viteConfig = ['vite.config.js', 'vite.config.ts', 'vite.config.mjs', 'vite.config.mts', 'vite.config.cjs'].find((name) => existsSync(join(repoRoot, name)));
  if (viteConfig) {
    try {
      const output = execFileSync(process.execPath, [driverPath, '--bundler-config', repoRoot], { cwd: repoRoot, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 1 << 24 });
      const config = JSON.parse(output);
      return {
        bundler: config.bundler,
        aliases: config.aliases,
        extensions: config.extensions ?? VITE_DEFAULT_EXTENSIONS,
        baseUrl: null,
        inputs: [...new Set([...config.inputs, viteConfig, 'package.json', ...configFiles, ...packageFiles])],
        packages,
        autoComponents: config.autoComponents,
        publicDir: config.publicDir,
        entries: config.entries,
        warnings,
      };
    } catch (error) {
      const lines = String(error.stderr || error.message).trim().split('\n');
      const reason = lines.find((line) => /^\w*Error\b/.test(line)) ?? lines[0];
      warnings.push(`${viteConfig} could not be loaded (${reason}${/Cannot find module/.test(reason) ? ', dependencies not installed' : ''}); module aliases come from tsconfig/jsconfig instead`);
    }
  }
  const aliases = [];
  let baseUrl = null;
  if (ts && configFiles.length) {
    const { config } = ts.readConfigFile(join(repoRoot, configFiles[0]), ts.sys.readFile);
    const options = config?.compilerOptions ?? {};
    const base = join(repoRoot, options.baseUrl ?? '.');
    if (options.baseUrl) baseUrl = base;
    for (const [pattern, targets] of Object.entries(options.paths ?? {})) {
      for (const target of targets) aliases.push({ find: pattern.replace(/\*$/, ''), wildcard: pattern.endsWith('*'), replacement: join(base, target.replace(/\*$/, '')) });
    }
  }
  return { bundler: configFiles[0] ?? 'none', aliases, extensions: VITE_DEFAULT_EXTENSIONS, baseUrl, inputs: [...new Set(['package.json', ...configFiles, ...(viteConfig ? [viteConfig] : []), ...packageFiles])], packages, autoComponents: {}, publicDir: null, entries: [], warnings };
}

/**
 * The repository's own packages (a pnpm/yarn/bun/npm workspace): `import { x } from '@acme/utils'` means a file
 * of `packages/utils`, found through that package's `exports`/`types`/`main` as a bundler would.
 */
function workspacePackages() {
  let files = [];
  try {
    files = execFileSync('git', ['ls-files', '-co', '--exclude-standard', '-z', '--', 'package.json', '*/package.json'], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 1 << 26 }).split('\0').filter(Boolean);
  } catch {
    return [];
  }
  const packages = [];
  for (const file of files) {
    try {
      const manifest = JSON.parse(readFileSync(join(repoRoot, file), 'utf8'));
      if (typeof manifest.name !== 'string') continue;
      const { name, exports, types, typings, source, module, main } = manifest;
      packages.push({ name, dir: dirname(file) === '.' ? '' : dirname(file), exports, entries: [source, types, typings, module, main].filter((entry) => typeof entry === 'string') });
    } catch {
      // not a readable manifest
    }
  }
  return packages;
}

/** Every file a condition object may pick, the source-like ones first (`types`, `development`, `import`...). */
const EXPORT_CONDITIONS = ['source', 'types', 'development', 'import', 'module', 'default', 'require', 'node', 'browser'];
function exportTargets(target) {
  if (typeof target === 'string') return [target];
  if (Array.isArray(target)) return target.flatMap(exportTargets);
  if (!target || typeof target !== 'object') return [];
  const keys = [...EXPORT_CONDITIONS.filter((key) => key in target), ...Object.keys(target).filter((key) => !EXPORT_CONDITIONS.includes(key))];
  return keys.flatMap((key) => exportTargets(target[key]));
}

/** Repo paths `subpath` (`.` or `./utils`) of a workspace package may mean. */
function packageTargets(entry, subpath) {
  const targets = [];
  let exportsMap = entry.exports;
  if (exportsMap !== undefined && exportsMap !== null) {
    if (typeof exportsMap !== 'object' || Array.isArray(exportsMap) || !Object.keys(exportsMap).some((key) => key.startsWith('.'))) exportsMap = { '.': exportsMap };
    if (subpath in exportsMap) targets.push(...exportTargets(exportsMap[subpath]));
    else {
      for (const [key, value] of Object.entries(exportsMap)) {
        const star = key.indexOf('*');
        if (star < 0) continue;
        const [prefix, suffix] = [key.slice(0, star), key.slice(star + 1)];
        if (!subpath.startsWith(prefix) || !subpath.endsWith(suffix) || subpath.length < prefix.length + suffix.length) continue;
        const matched = subpath.slice(prefix.length, subpath.length - suffix.length);
        targets.push(...exportTargets(value).map((target) => target.replaceAll('*', matched)));
      }
    }
  }
  if (subpath === '.') targets.push(...entry.entries, 'index', 'src/index');
  else targets.push(subpath);
  const paths = [];
  for (const target of targets) {
    const path = join(entry.dir, target).split(sep).join('/');
    paths.push(path.replace(/\.d\.[cm]?ts$/, ''));
    // Built output named in the manifest (`dist/index.js`) comes from the package's sources (`src/index.ts`).
    const built = path.match(/^(.*?)(?:^|\/)(?:dist|build|lib|out)\/(.*)$/);
    if (built) paths.push(`${built[1] ? `${built[1]}/` : ''}src/${built[2]}`.replace(/\.d\.[cm]?ts$/, '').replace(/\.[cm]?js$/, ''));
  }
  return [...new Set(paths)];
}

function workspaceCandidates(specifier) {
  let best = null;
  for (const entry of resolution.packages ?? []) {
    if ((specifier === entry.name || specifier.startsWith(`${entry.name}/`)) && (!best || entry.name.length > best.name.length)) best = entry;
  }
  if (!best) return null;
  return packageTargets(best, specifier === best.name ? '.' : `.${specifier.slice(best.name.length)}`);
}

function compileResolution(portable) {
  return { ...portable, portable, aliases: portable.aliases.map((alias) => (alias.pattern === undefined ? alias : { ...alias, regex: new RegExp(alias.pattern, alias.flags) })) };
}

/**
 * Script files of the app; the bundler's public folder is copied as-is (often prebuilt bundles) and never imported.
 * Whether a file is a bundle is taken from the saved facts while the file is unchanged, instead of read again.
 */
function listFiles(saved = null) {
  const output = execFileSync('git', ['ls-files', '-co', '--exclude-standard', '-z'], { cwd: repoRoot, encoding: 'utf8', maxBuffer: 1 << 28 });
  const files = output.split('\0').filter((file) => isScriptFile(file));
  const verdicts = new Map([...(saved?.files ?? []).map((json) => [json.path, [json.stamp, false]]), ...(saved?.bundles ?? []).map(([path, stamp]) => [path, [stamp, true]])]);
  const bundles = files.filter((file) => {
    const [stamp, bundle] = verdicts.get(file) ?? [];
    return stamp && JSON.stringify(stamp) === JSON.stringify(fileStamp(file)) ? bundle : isBundle(file);
  });
  skippedBundles = bundles;
  return files.filter((file) => !bundles.includes(file));
}

/** Files committed after a build (`public/vendor/app.js`, a minified library): they call nothing of the source. */
let skippedBundles = [];
function isBundle(path) {
  let head = '';
  try {
    const descriptor = openSync(join(repoRoot, path), 'r');
    const buffer = Buffer.alloc(65536);
    head = buffer.subarray(0, readSync(descriptor, buffer, 0, buffer.length, 0)).toString('utf8');
    closeSync(descriptor);
  } catch {
    return false;
  }
  return head.split('\n').some((line) => line.length > 3000) || hasMangledNames(head);
}

/** A minified build reformatted later: nearly every parameter and variable is one letter (`(t, e, o) =>`). */
function hasMangledNames(head) {
  const names = [];
  for (const match of head.matchAll(/\bfunction\b\s*[\w$]*\s*\(([^()]*)\)|\(([^()]*)\)\s*=>/g)) {
    for (const parameter of (match[1] ?? match[2]).split(',')) names.push(parameter.trim().match(/^[A-Za-z_$][\w$]*/)?.[0]);
  }
  for (const match of head.matchAll(/\b(?:var|let|const)\s+([A-Za-z_$][\w$]*)/g)) names.push(match[1]);
  const named = names.filter(Boolean);
  return named.length >= 100 && named.filter((name) => name.length === 1).length / named.length > 0.7;
}

function isScriptFile(path) {
  return SCRIPT_EXTENSIONS.has(extname(path)) && !(resolution.publicDir && path.startsWith(`${resolution.publicDir}/`));
}

function toRepoPath(absolute) {
  const path = relative(repoRoot, absolute).split(sep).join('/');
  return path.startsWith('..') ? null : path;
}

/** Repo-relative paths (before extension guessing) a specifier may point at, in the bundler's order. */
function specifierCandidates(fromPath, specifier) {
  if (specifier.startsWith('.')) return [toRepoPath(join(repoRoot, dirname(fromPath), specifier))].filter(Boolean);
  const candidates = [];
  for (const alias of resolution.aliases) {
    let replaced = null;
    if (alias.regex) {
      if (alias.regex.test(specifier)) replaced = specifier.replace(alias.regex, alias.replacement);
    } else if (specifier === alias.find || (alias.wildcard ? specifier.startsWith(alias.find) : specifier.startsWith(`${alias.find}/`))) {
      replaced = alias.replacement + specifier.slice(alias.find.length);
    }
    if (replaced === null) continue;
    // Vite reads an aliased `/x` as a path from the project root before trying it as an absolute path.
    if (replaced.startsWith('/') && !replaced.startsWith(repoRoot)) candidates.push(join(repoRoot, replaced));
    candidates.push(resolvePath(repoRoot, replaced));
    if (alias.wildcard === undefined) break;
  }
  if (specifier.startsWith('/')) candidates.push(join(repoRoot, specifier));
  if (!candidates.length) {
    const workspace = workspaceCandidates(specifier);
    if (workspace) return workspace;
  }
  if (resolution.baseUrl && !candidates.length) candidates.push(join(resolution.baseUrl, specifier));
  return candidates.map(toRepoPath).filter(Boolean);
}

/** TypeScript's ESM style names the emitted file: `./util.js` is the source `./util.ts`. */
const EMITTED_EXTENSIONS = { '.js': ['.ts', '.tsx'], '.jsx': ['.tsx'], '.mjs': ['.mts'], '.cjs': ['.cts'] };

function resolveSpecifier(fromPath, specifier) {
  const bases = specifierCandidates(fromPath, specifier);
  for (const base of bases) {
    if (known.has(base)) return { module: base, bases };
    const emitted = EMITTED_EXTENSIONS[extname(base)];
    for (const extension of emitted ?? []) if (known.has(base.slice(0, -extname(base).length) + extension)) return { module: base.slice(0, -extname(base).length) + extension, bases };
    for (const extension of resolution.extensions) if (known.has(base + extension)) return { module: base + extension, bases };
    for (const extension of resolution.extensions) if (known.has(`${base}/index${extension}`)) return { module: `${base}/index${extension}`, bases };
  }
  return { module: null, bases };
}

/** The specifier bases whose meaning changes when this file appears or disappears. */
function basesOf(path) {
  const withoutExtension = path.slice(0, path.length - extname(path).length);
  return basename(withoutExtension) === 'index' ? [path, withoutExtension, dirname(withoutExtension)] : [path, withoutExtension];
}

function globToRegExp(pattern) {
  let source = '';
  for (let index = 0; index < pattern.length; index++) {
    const character = pattern[index];
    if (character === '*' && pattern[index + 1] === '*') {
      const slash = pattern[index + 2] === '/';
      source += slash ? '(?:.*/)?' : '.*';
      index += slash ? 2 : 1;
    } else if (character === '*') source += '[^/]*';
    else if (character === '?') source += '[^/]';
    else if (character === '{') source += '(?:';
    else if (character === '}') source += ')';
    else if (character === ',') source += '|';
    else source += character.replace(/[.+^$()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${source}$`);
}

/**
 * An MDX page as TSX on the same lines: its `import`/`export` blocks as written, and of the prose only the
 * component tags (`<Callout>` becomes `<Callout />;`), which is all of it that can call this repository's code.
 */
function mdxAsTsx(text) {
  let inEsm = false;
  let inFence = false;
  return text
    .split('\n')
    .map((line) => {
      if (/^(```|~~~)/.test(line)) inFence = !inFence;
      if (inFence) return '';
      if (/^(import|export)\b/.test(line)) inEsm = true;
      else if (!line.trim()) inEsm = false;
      if (inEsm) return line;
      return [...line.matchAll(/<([A-Z][\w.]*)/g)].map((match) => `<${match[1]} />;`).join(' ');
    })
    .join('\n');
}

function scriptKind(lang) {
  return { ts: ts.ScriptKind.TS, tsx: ts.ScriptKind.TSX, jsx: ts.ScriptKind.JSX }[lang] ?? ts.ScriptKind.JS;
}

function propertyName(name) {
  if (name && (ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isPrivateIdentifier(name) || ts.isNumericLiteral(name))) return name.text;
  return null;
}

function bindingNames(name, into = []) {
  if (ts.isIdentifier(name)) into.push(name);
  else if (ts.isObjectBindingPattern(name) || ts.isArrayBindingPattern(name)) {
    for (const element of name.elements) if (ts.isBindingElement(element)) bindingNames(element.name, into);
  }
  return into;
}

/** An expression of an ESTree (Svelte's AST), not markup. */
function isEstreeExpression(node) {
  return typeof node.type === 'string' && (node.type.endsWith('Expression') || node.type === 'Identifier' || node.type === 'Literal' || node.type === 'TemplateLiteral');
}

/** Names an ESTree binding pattern declares. */
function estreeNames(pattern, into = []) {
  if (!pattern) return into;
  if (pattern.type === 'Identifier') into.push(pattern.name);
  else if (pattern.type === 'ObjectPattern') for (const property of pattern.properties) estreeNames(property.type === 'RestElement' ? property.argument : property.value, into);
  else if (pattern.type === 'ArrayPattern') for (const element of pattern.elements) estreeNames(element, into);
  else if (pattern.type === 'RestElement') estreeNames(pattern.argument, into);
  else if (pattern.type === 'AssignmentPattern') estreeNames(pattern.left, into);
  return into;
}

/** The first argument of a call that initialises a variable: `const save = useCallback(() => ..., deps)`. */
function wrappedIn(node) {
  const call = node.parent;
  return Boolean(call) && ts.isCallExpression(call) && call.arguments[0] === node && ts.isVariableDeclaration(call.parent) && ts.isIdentifier(call.parent.name);
}

/** Whether a function body has `return name` of its own (not inside a nested function). */
function returnsName(body, name) {
  const visit = (node) => {
    if (ts.isReturnStatement(node)) return !!node.expression && ts.isIdentifier(node.expression) && node.expression.text === name;
    return !ts.isFunctionLike(node) && !!ts.forEachChild(node, visit);
  };
  return !!ts.forEachChild(body, visit);
}

/** `object.name = () => ...` */
function assignedToMember(node) {
  const parent = node.parent;
  return Boolean(parent) && ts.isBinaryExpression(parent) && parent.operatorToken.kind === ts.SyntaxKind.EqualsToken && parent.right === node && ts.isPropertyAccessExpression(parent.left);
}

function unwrapExpression(node) {
  while (node && (ts.isParenthesizedExpression(node) || ts.isAwaitExpression(node) || ts.isAsExpression(node) || ts.isNonNullExpression(node) || ts.isSatisfiesExpression(node))) node = node.expression;
  return node;
}

function isTopLevelVariable(node) {
  return ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && ts.isVariableStatement(node.parent?.parent) && ts.isSourceFile(node.parent.parent.parent);
}

function isFunctionValue(node) {
  return Boolean(node) && (ts.isArrowFunction(node) || ts.isFunctionExpression(node) || ts.isClassExpression(node));
}

function hasModifier(node, kind) {
  return ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((modifier) => modifier.kind === kind);
}

/** The object literal of `export default {...}` or `export default defineComponent({...})`. */
function defaultExportObject(expression) {
  if (ts.isObjectLiteralExpression(expression)) return expression;
  if (ts.isCallExpression(expression) && expression.arguments[0] && ts.isObjectLiteralExpression(expression.arguments[0])) return expression.arguments[0];
  return null;
}

const TARGETS = ['module', 'import', 'own', 'global', 'member', 'result', 'lost'];
/** The class a declared type names: `Recorder`, also `Recorder | undefined` or `Recorder | null`. */
function classTypeOf(type) {
  if (type && ts.isUnionTypeNode(type)) {
    const named = type.types.filter((member) => member.kind !== ts.SyntaxKind.UndefinedKeyword && !(ts.isLiteralTypeNode(member) && member.literal.kind === ts.SyntaxKind.NullKeyword));
    type = named.length === 1 ? named[0] : null;
  }
  return type && ts.isTypeReferenceNode(type) && ts.isIdentifier(type.typeName) ? type : null;
}

const KINDS = ['call', 'reference', 'component', 'template', 'import', 'lazy import', 'require', 'returned', 'handed', 'passed'];
/**
 * A reference is seven numbers: target, line, from, kind, imported name, member, and the property it is
 * stored under (`{ mapResponse: format }`), strings as indexes into `strings`.
 */
const REFERENCE_SIZE = 7;

/** Everything a file says about its symbols and references, independent of every other file. */
/** (modification time, size) of a repository file, null when it is gone. */
function fileStamp(path) {
  try {
    const status = statSync(join(repoRoot, path));
    return [status.mtimeMs, status.size];
  } catch {
    return null;
  }
}

class FileFacts {
  constructor(path) {
    this.path = path;
    /** the file's (mtime, size) when it was read: a saved fact is reused only while it holds */
    this.stamp = null;
    this.isComponent = /\.(vue|svelte)$/.test(path);
    this.topSymbol = this.isComponent ? 'default' : '(module)';
    this.isTest = TEST_PATH.test(path);
    this.strings = [];
    this.stringIds = new Map();
    /** key => [{line, end}] */
    this.declarations = new Map();
    /** keys Vue itself calls once the component is used (lifecycle, computed, watch) */
    this.componentHooks = new Set();
    /** keys reachable as `object.name`: class and object members, what `defineExpose` exposes, CommonJS exports */
    this.memberKeys = new Set();
    /** Svelte snippets: handed to a child as a prop and rendered there, so any `{@render name()}` may be one */
    this.snippetKeys = new Set();
    /** top-level variables with a computed value: evaluated whenever the module is loaded */
    this.loadTimeKeys = new Set();
    /** class key => {key} | {module, name}: the class it extends */
    this.classBases = new Map();
    /** function key => [[property, key, written in a literal it returns]]: what calling it gives access to */
    this.returnedMembers = new Map();
    /** key => {from, line}: a function written inside an object handed to other code, which calls it back */
    this.handedOver = new Map();
    this.isCommonJs = false;
    /** exported name => {local} | {module, name} */
    this.exports = new Map();
    this.starExports = [];
    /** member of the default-exported object => {local} | {module, name} */
    this.defaultMembers = new Map();
    /** `m:<module>`, `n:<name>` (unbound or untyped) or `o:<key>` (same file) => flat references */
    this.references = new Map();
    this.importBases = new Set();
    /** [{positive: RegExp[], negative: RegExp[]}] */
    this.globs = [];
    this.warnings = [];
  }

  string(value) {
    if (value === null || value === undefined) return -1;
    let id = this.stringIds.get(value);
    if (id === undefined) {
      id = this.strings.length;
      this.strings.push(value);
      this.stringIds.set(value, id);
    }
    return id;
  }

  addReference(indexKey, target, line, from, kind, name = null, member = null, property = null) {
    let list = this.references.get(indexKey);
    if (!list) this.references.set(indexKey, (list = []));
    list.push(TARGETS.indexOf(target), line, this.string(from), KINDS.indexOf(kind), this.string(name), this.string(member), this.string(property));
  }

  *referencesUnder(indexKey) {
    const list = this.references.get(indexKey) ?? [];
    for (let index = 0; index < list.length; index += REFERENCE_SIZE) {
      yield { to: TARGETS[list[index]], line: list[index + 1], from: this.strings[list[index + 2]], kind: KINDS[list[index + 3]], name: this.strings[list[index + 4]] ?? null, member: this.strings[list[index + 5]] ?? null, property: this.strings[list[index + 6]] ?? null };
    }
  }

  reexportedModules() {
    return [...this.exports.values(), ...this.defaultMembers.values()].filter((value) => value.module).map((value) => value.module).concat(this.starExports);
  }

  toJSON() {
    return {
      path: this.path,
      stamp: this.stamp,
      strings: this.strings,
      declarations: [...this.declarations],
      componentHooks: [...this.componentHooks],
      memberKeys: [...this.memberKeys],
      snippetKeys: [...this.snippetKeys],
      loadTimeKeys: [...this.loadTimeKeys],
      handedOver: [...this.handedOver],
      returnedMembers: [...this.returnedMembers],
      classBases: [...this.classBases],
      isCommonJs: this.isCommonJs,
      exports: [...this.exports],
      starExports: this.starExports,
      defaultMembers: [...this.defaultMembers],
      references: [...this.references],
      importBases: [...this.importBases],
      globs: this.globs.map((glob) => ({ positive: glob.positive.map((regex) => regex.source), negative: glob.negative.map((regex) => regex.source) })),
      warnings: this.warnings,
    };
  }

  static fromJSON(json) {
    const facts = new FileFacts(json.path);
    facts.stamp = json.stamp;
    facts.strings = json.strings;
    facts.stringIds = null;
    facts.declarations = new Map(json.declarations);
    facts.componentHooks = new Set(json.componentHooks);
    facts.memberKeys = new Set(json.memberKeys);
    facts.snippetKeys = new Set(json.snippetKeys);
    facts.loadTimeKeys = new Set(json.loadTimeKeys);
    facts.handedOver = new Map(json.handedOver);
    facts.returnedMembers = new Map(json.returnedMembers);
    facts.classBases = new Map(json.classBases);
    facts.isCommonJs = json.isCommonJs;
    facts.exports = new Map(json.exports);
    facts.starExports = json.starExports;
    facts.defaultMembers = new Map(json.defaultMembers);
    facts.references = new Map(json.references);
    facts.importBases = new Set(json.importBases);
    facts.globs = json.globs.map((glob) => ({ positive: glob.positive.map((source) => new RegExp(source)), negative: glob.negative.map((source) => new RegExp(source)) }));
    facts.warnings = json.warnings;
    return facts;
  }
}

class FileReader {
  constructor(facts, text) {
    this.facts = facts;
    this.text = text;
    this.scopeCache = new Map();
    this.keyCache = new Map();
    this.moduleBindings = new Map();
    this.template = null;
  }

  read() {
    const sources = this.scriptBlocks().map((block) => ({
      source: ts.createSourceFile(this.facts.path, '\n'.repeat(block.line - 1) + block.code, ts.ScriptTarget.Latest, true, scriptKind(block.lang)),
      setup: block.setup,
    }));
    // `<script>` and `<script setup>` of one component share their module scope.
    for (const { source } of sources) this.collectModuleBindings(source);
    // Declarations first: `this.save()` may come before `save()` is written.
    for (const { source } of sources) this.walk(source, (node) => this.visitDeclaration(node, source));
    for (const { source, setup } of sources) {
      this.source = source;
      if (setup) this.setupSource = source;
      this.walk(source, (node) => this.visitReference(node));
    }
    if (this.template) this.visitTemplate(this.template, new Set());
    if (this.svelteTemplate) {
      this.declareSnippets(this.svelteTemplate);
      this.visitSvelte(this.svelteTemplate, new Set());
    }
    this.facts.stringIds = null;
  }

  walk(node, visitor) {
    if (ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node) || (ts.isTypeNode(node) && !ts.isExpressionWithTypeArguments(node))) return;
    visitor(node);
    ts.forEachChild(node, (child) => this.walk(child, visitor));
  }

  scriptBlocks() {
    if (this.facts.path.endsWith('.svelte')) return this.svelteScriptBlocks();
    if (this.facts.path.endsWith('.mdx')) return [{ code: mdxAsTsx(this.text), line: 1, lang: 'tsx', setup: false }];
    if (!this.facts.isComponent) return [{ code: this.text, line: 1, lang: extname(this.facts.path).slice(1).replace(/^[mc](?=ts|js)/, ''), setup: false }];
    if (!vue) {
      this.facts.warnings.push(`vue/compiler-sfc is not installed; ${this.facts.path} was not analysed`);
      return [];
    }
    const { descriptor, errors } = vue.parse(this.text, { filename: this.facts.path });
    if (errors.length) this.facts.warnings.push(`${this.facts.path}: ${String(errors[0].message ?? errors[0])}`);
    this.template = descriptor.template?.ast ?? null;
    this.indexLines();
    // Lines from offsets: the compiler's own `loc.line` runs ahead after an attribute written over several lines.
    return [descriptor.script, descriptor.scriptSetup].filter(Boolean).map((block) => ({ code: block.content, line: this.lineOfOffset(block.loc.start.offset), lang: block.lang ?? 'js', setup: block === descriptor.scriptSetup }));
  }

  /** `<script module>` and the instance `<script>` of a Svelte component; the markup is read afterwards. */
  svelteScriptBlocks() {
    if (!svelte) {
      this.facts.warnings.push(`svelte/compiler is not installed; ${this.facts.path} was not analysed`);
      return [];
    }
    const ast = svelte.parse(this.text, { modern: true });
    this.svelteTemplate = ast.fragment;
    this.indexLines();
    return [ast.module, ast.instance].filter(Boolean).map((script) => ({
      code: this.text.slice(script.content.start, script.content.end),
      line: this.lineOfOffset(script.content.start),
      lang: script.attributes?.find((attribute) => attribute.name === 'lang')?.value?.[0]?.data ?? 'js',
      setup: script === ast.instance,
    }));
  }

  indexLines() {
    this.lineStarts = [0];
    for (let index = 0; index < this.text.length; index++) if (this.text[index] === '\n') this.lineStarts.push(index + 1);
  }

  lineOfOffset(offset) {
    let low = 0;
    let high = this.lineStarts.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (this.lineStarts[middle] <= offset) low = middle;
      else high = middle - 1;
    }
    return low + 1;
  }

  /**
   * A Svelte template: every expression, component tag and action, with `{#each}`, `{#snippet}`, `{#await}`,
   * `{@const}` and `let:` names as template locals.
   */
  visitSvelte(node, locals) {
    if (!node || typeof node !== 'object') return;
    if (Array.isArray(node)) {
      let current = locals;
      for (const child of node) {
        if (child?.type === 'ConstTag') {
          current = new Set(current);
          for (const declarator of child.declaration.declarations) {
            for (const name of estreeNames(declarator.id)) current.add(name);
            this.svelteExpression(declarator.init, current);
          }
          continue;
        }
        this.visitSvelte(child, current);
      }
      return;
    }
    if (isEstreeExpression(node)) return this.svelteExpression(node, locals);
    const inner = (...patterns) => {
      const set = new Set(locals);
      for (const pattern of patterns) for (const name of estreeNames(pattern)) set.add(name);
      return set;
    };
    switch (node.type) {
      case 'EachBlock': {
        this.svelteExpression(node.expression, locals);
        const scoped = inner(node.context, typeof node.index === 'string' ? { type: 'Identifier', name: node.index } : node.index);
        this.svelteExpression(node.key, scoped);
        this.visitSvelte(node.body, scoped);
        return this.visitSvelte(node.fallback, locals);
      }
      case 'SnippetBlock':
        return this.visitSvelte(node.body, inner(...(node.parameters ?? [])));
      case 'AwaitBlock':
        this.svelteExpression(node.expression, locals);
        this.visitSvelte(node.pending, locals);
        this.visitSvelte(node.then, inner(node.value));
        return this.visitSvelte(node.catch, inner(node.error));
      case 'Component':
      case 'InlineComponent': {
        const scoped = inner(...(node.attributes ?? []).filter((attribute) => attribute.type === 'LetDirective').map((attribute) => attribute.expression ?? { type: 'Identifier', name: attribute.name }));
        if (!locals.has(node.name.split('.')[0])) {
          const [name, member = null] = node.name.split('.');
          const binding = this.templateBinding(name);
          this.record(binding, node.name, member, this.lineOfOffset(node.start), 'default', 'component');
        }
        for (const attribute of node.attributes ?? []) if (attribute.type !== 'LetDirective') this.visitSvelte(attribute, locals);
        return this.visitSvelte(node.fragment, scoped);
      }
      case 'UseDirective':
      case 'TransitionDirective':
      case 'AnimateDirective': {
        // `use:tooltip` names a function of the component's scope.
        this.record(this.templateBinding(node.name), node.name, null, this.lineOfOffset(node.start), 'default', 'template');
        return this.svelteExpression(node.expression, locals);
      }
    }
    for (const [key, value] of Object.entries(node)) {
      if (key !== 'metadata' && key !== 'loc' && key !== 'name_loc' && value && typeof value === 'object') this.visitSvelte(value, locals);
    }
  }

  /**
   * `{#snippet row(entry)}`: a function of the component, rendered with `{@render row(x)}` or handed to a child
   * as a prop and rendered there (then found by name, calls only).
   */
  declareSnippets(node) {
    if (!node || typeof node !== 'object') return;
    if (Array.isArray(node)) return node.forEach((child) => this.declareSnippets(child));
    if (node.type === 'SnippetBlock' && node.expression?.name) {
      this.facts.declarations.set(node.expression.name, [{ line: this.lineOfOffset(node.start), end: this.lineOfOffset(node.end) }]);
      this.facts.snippetKeys.add(node.expression.name);
    }
    for (const [key, value] of Object.entries(node)) if (key !== 'metadata' && value && typeof value === 'object') this.declareSnippets(value);
  }

  svelteExpression(node, locals) {
    if (!node || typeof node.start !== 'number') return;
    this.visitTemplateExpression(this.text.slice(node.start, node.end), this.lineOfOffset(node.start), locals);
  }

  line(node, source = this.source) {
    return source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
  }

  /** The key of a function-like or class node, `null` for anonymous ones (their code belongs to the enclosing key). */
  declarationKey(node) {
    if (this.keyCache.has(node)) return this.keyCache.get(node);
    let name = null;
    let ownerClass = null;
    const parent = node.parent;
    if (ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node)) name = node.name?.text ?? (hasModifier(node, ts.SyntaxKind.DefaultKeyword) ? 'default' : null);
    else if (ts.isMethodDeclaration(node) || ts.isGetAccessorDeclaration(node) || ts.isSetAccessorDeclaration(node)) {
      name = propertyName(node.name);
      if (ts.isClassLike(parent)) ownerClass = parent;
    } else if (ts.isConstructorDeclaration(node)) {
      name = 'constructor';
      ownerClass = parent;
    } else if (isFunctionValue(node)) {
      if (ts.isClassExpression(node) && node.name) name = node.name.text;
      else if (ts.isVariableDeclaration(parent) && ts.isIdentifier(parent.name)) name = parent.name.text;
      // `const save = useCallback(async () => ..., [])`: the wrapped function goes by the variable's name.
      else if (wrappedIn(node)) name = node.parent.parent.name.text;
      // `debounced.cancel = () => ...`: reached as a member of that object.
      else if (assignedToMember(node)) name = node.parent.left.name.text;
      else if (ts.isPropertyAssignment(parent)) name = propertyName(parent.name);
      else if (ts.isPropertyDeclaration(parent)) {
        name = propertyName(parent.name);
        ownerClass = parent.parent;
      } else if (ts.isExportAssignment(parent)) name = 'default';
    } else if (isTopLevelVariable(node)) name = node.name.text;
    let key = null;
    if (name !== null) {
      if (ownerClass) {
        const classKey = this.declarationKey(ownerClass);
        key = classKey === null ? null : `${classKey}.${name}`;
      } else {
        const anchor = isFunctionValue(node) && ts.isVariableDeclaration(parent) ? parent : wrappedIn(node) ? parent.parent : node;
        const owner = this.enclosingKey(anchor);
        key = owner === this.facts.topSymbol ? name : `${owner}.${name}`;
      }
    }
    this.keyCache.set(node, key);
    return key;
  }

  /** The key of the nearest named function or class around a node; top-level code belongs to the file. */
  enclosingKey(node) {
    for (let current = node.parent; current && !ts.isSourceFile(current); current = current.parent) {
      if (ts.isFunctionLike(current) || ts.isClassLike(current) || isTopLevelVariable(current)) {
        const key = this.declarationKey(current);
        if (key !== null) return key;
      }
    }
    return this.facts.topSymbol;
  }

  collectModuleBindings(source) {
    for (const statement of source.statements) {
      if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier) || statement.importClause?.isTypeOnly) continue;
      const { module, bases } = resolveSpecifier(this.facts.path, statement.moduleSpecifier.text);
      for (const base of bases) this.facts.importBases.add(base);
      if (module) this.facts.addReference(`m:${module}`, 'module', this.line(statement, source), this.facts.topSymbol, 'import');
      const clause = statement.importClause;
      if (!clause) continue;
      // A specifier that names this repository (a path, an alias, a workspace package) but no file of it: its
      // calls are kept by name, unresolved, instead of vanishing like a package's.
      const internal = statement.moduleSpecifier.text.startsWith('.') || bases.length > 0;
      const binding = (name) => (module ? { kind: 'import', module, name } : internal ? { kind: 'lost', name } : { kind: 'package' });
      if (clause.name) this.moduleBindings.set(clause.name.text, binding('default'));
      const named = clause.namedBindings;
      if (named && ts.isNamespaceImport(named)) this.moduleBindings.set(named.name.text, binding('*'));
      if (named && ts.isNamedImports(named)) for (const element of named.elements) this.moduleBindings.set(element.name.text, binding((element.propertyName ?? element.name).text));
    }
  }

  /** Names bound directly by a scope node (with `var` hoisted to its function). */
  scopeBindings(scope) {
    let bindings = this.scopeCache.get(scope);
    if (bindings) return bindings;
    bindings = new Map();
    const bind = (identifier, declaration) => bindings.set(identifier.text, declaration);
    const bindStatements = (statements) => {
      for (const statement of statements) {
        if ((ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement) || ts.isEnumDeclaration(statement)) && statement.name) bind(statement.name, statement);
        if (ts.isVariableStatement(statement) && statement.declarationList.flags & ts.NodeFlags.BlockScoped) {
          for (const declaration of statement.declarationList.declarations) for (const name of bindingNames(declaration.name)) bind(name, declaration);
        }
      }
    };
    const hoistVars = (node) => {
      ts.forEachChild(node, (child) => {
        if (ts.isFunctionLike(child)) return;
        if (ts.isVariableDeclarationList(child) && !(child.flags & ts.NodeFlags.BlockScoped)) {
          for (const declaration of child.declarations) for (const name of bindingNames(declaration.name)) bind(name, declaration);
        }
        hoistVars(child);
      });
    };
    if (ts.isFunctionLike(scope)) {
      for (const parameter of scope.parameters ?? []) for (const name of bindingNames(parameter.name)) bind(name, parameter);
      if (ts.isFunctionExpression(scope) && scope.name) bind(scope.name, scope);
      if (scope.body) hoistVars(scope.body);
    } else if (ts.isClassExpression(scope) && scope.name) bind(scope.name, scope);
    else if (ts.isBlock(scope) || ts.isModuleBlock(scope)) bindStatements(scope.statements);
    else if (ts.isCaseBlock(scope)) bindStatements(scope.clauses.flatMap((clause) => [...clause.statements]));
    else if ((ts.isForStatement(scope) || ts.isForInStatement(scope) || ts.isForOfStatement(scope)) && scope.initializer && ts.isVariableDeclarationList(scope.initializer)) {
      for (const declaration of scope.initializer.declarations) for (const name of bindingNames(declaration.name)) bind(name, declaration);
    } else if (ts.isCatchClause(scope) && scope.variableDeclaration) {
      for (const name of bindingNames(scope.variableDeclaration.name)) bind(name, scope.variableDeclaration);
    } else if (ts.isSourceFile(scope)) {
      bindStatements(scope.statements);
      hoistVars(scope);
    }
    this.scopeCache.set(scope, bindings);
    return bindings;
  }

  /** What a name refers to at a node: a function or class here (`own`), an import, a plain local, or nothing (global). */
  bindingOf(node, name) {
    for (let current = node.parent; current; current = current.parent) {
      const declaration = this.scopeBindings(current).get(name);
      if (!declaration) continue;
      let value = ts.isVariableDeclaration(declaration) ? declaration.initializer : declaration;
      if (value && ts.isCallExpression(value) && value.arguments[0] && isFunctionValue(value.arguments[0]) && wrappedIn(value.arguments[0])) value = value.arguments[0];
      const key = value && (ts.isFunctionLike(value) || ts.isClassLike(value)) ? this.declarationKey(value) : isTopLevelVariable(declaration) ? this.declarationKey(declaration) : null;
      return key ? { kind: 'own', key, declaration } : { kind: 'local', declaration };
    }
    return this.moduleBindings.get(name) ?? { kind: 'global' };
  }

  isReference(node) {
    const parent = node.parent;
    if (!parent) return false;
    if (ts.isPropertyAccessExpression(parent) && parent.name === node) return false;
    if (parent.name === node && !ts.isShorthandPropertyAssignment(parent)) return false;
    if (ts.isQualifiedName(parent) || ts.isPartOfTypeNode(node)) return false;
    if (ts.isBindingElement(parent) && parent.propertyName === node) return false;
    if (ts.isImportSpecifier(parent) || ts.isImportClause(parent) || ts.isNamespaceImport(parent) || ts.isExportSpecifier(parent)) return false;
    if (ts.isLabeledStatement(parent) || ts.isBreakOrContinueStatement(parent) || ts.isMetaProperty(parent) || ts.isJsxClosingElement(parent)) return false;
    if ((ts.isJsxOpeningElement(parent) || ts.isJsxSelfClosingElement(parent)) && /^[a-z]/.test(node.text)) return false;
    return true;
  }

  isCallee(node) {
    const parent = node.parent;
    return Boolean(parent) && (ts.isCallExpression(parent) || ts.isNewExpression(parent) || ts.isTaggedTemplateExpression(parent)) && (parent.expression ?? parent.tag) === node;
  }

  /** The member read right after an expression (`api.get`), when there is one. */
  memberAfter(node) {
    const parent = node.parent;
    return parent && ts.isPropertyAccessExpression(parent) && parent.expression === node ? parent.name.text : null;
  }

  record(binding, name, member, line, from, kind, property = null) {
    if (binding.kind === 'import') this.facts.addReference(`m:${binding.module}`, 'import', line, from, kind, binding.name, member, property);
    else if (binding.kind === 'own') this.facts.addReference(`o:${binding.key}`, 'own', line, from, kind, null, null, property);
    else if (binding.kind === 'global') this.facts.addReference(`n:${name}`, 'global', line, from, kind);
    else if (binding.kind === 'lost') this.facts.addReference(`n:${binding.name === '*' || binding.name === 'default' ? name : binding.name}`, 'lost', line, from, kind);
  }

  /** `this.name` inside a class, or in a component's options: that member when it is declared here. */
  thisMember(node, name) {
    for (let current = node.parent; current && !ts.isSourceFile(current); current = current.parent) {
      if (ts.isClassLike(current)) {
        const classKey = this.declarationKey(current);
        return classKey && this.facts.declarations.has(`${classKey}.${name}`) ? { kind: 'own', key: `${classKey}.${name}` } : null;
      }
      if (ts.isFunctionDeclaration(current)) return null;
    }
    return this.facts.isComponent && this.facts.declarations.has(name) ? { kind: 'own', key: name } : null;
  }

  visitReference(node) {
    if (ts.isIdentifier(node)) {
      if (!this.isReference(node)) return;
      const binding = this.bindingOf(node, node.text);
      const heldAs = this.heldAs(node);
      if (binding.kind === 'local' || binding.kind === 'own') {
        const result = this.resultOf(node, binding.declaration);
        if (result) this.recordResult(result, this.line(node), this.enclosingKey(node), heldAs);
        // `const { snooze } = useNotifications()`, `({ onCreated }) => ...`: calling it (or passing it on) uses
        // whatever the object held under that name. Recorded by name too when the result is followed: the
        // function may return an object no literal shows (`useContext(...)`).
        // The producer goes with it: when that function's result is followed, the name lookup is not needed.
        const property = this.destructuredProperty(binding.declaration, node.text);
        const producer = result?.producer.kind === 'import' ? [result.producer.module, result.producer.name, null] : result ? [this.facts.path, null, result.producer.key] : [];
        if (property) this.facts.addReference(`n:${property}`, 'member', this.line(node), this.enclosingKey(node), this.isCallee(node) ? 'call' : 'passed', ...producer);
        if (binding.kind === 'local') return;
      }
      const member = binding.kind === 'import' ? this.memberAfter(node) : null;
      const isTag = (ts.isJsxOpeningElement(node.parent) || ts.isJsxSelfClosingElement(node.parent)) && node.parent.tagName === node;
      const holder = this.objectHolder(node);
      const kind = isTag ? 'component' : holder ?? (this.isCallee(member ? node.parent : node) ? 'call' : 'reference');
      this.record(binding, node.text, member, this.line(node), this.enclosingKey(node), kind, heldAs);
    } else if (ts.isPropertyAccessExpression(node)) {
      this.visitMember(node);
      if (ts.isIdentifier(node.expression) && (node.expression.text === 'module' || node.expression.text === 'exports') && this.bindingOf(node.expression, node.expression.text).kind === 'global') this.facts.isCommonJs = true;
    } else if (ts.isCallExpression(node)) {
      this.visitLoadingCall(node);
      this.visitExpose(node);
    } else if (ts.isReturnStatement(node) || (ts.isArrowFunction(node) && !ts.isBlock(node.body))) {
      this.visitReturnedObject(node);
    }
  }

  /** `defineExpose({ save })`: the parent component may call `childRef.value.save()`. */
  visitExpose(node) {
    if (!ts.isIdentifier(node.expression) || node.expression.text !== 'defineExpose' || !node.arguments[0] || !ts.isObjectLiteralExpression(node.arguments[0])) return;
    for (const property of node.arguments[0].properties) {
      const value = ts.isShorthandPropertyAssignment(property) ? property.name : ts.isPropertyAssignment(property) && ts.isIdentifier(property.initializer) ? property.initializer : null;
      const binding = value ? this.bindingOf(value, value.text) : null;
      if (binding?.kind === 'own') this.facts.memberKeys.add(binding.key);
    }
  }

  visitMember(node) {
    // `debounced.cancel = ...` stores into the member; it does not use it.
    if (ts.isBinaryExpression(node.parent) && node.parent.operatorToken.kind === ts.SyntaxKind.EqualsToken && node.parent.left === node) return;
    const object = node.expression;
    const name = node.name.text;
    if (ts.isIdentifier(object) && this.isReference(object)) {
      const binding = this.bindingOf(object, object.text);
      if (binding.kind === 'import' || binding.kind === 'package') return;
      if (binding.kind === 'lost') return this.facts.addReference(`n:${name}`, 'lost', this.line(node.name), this.enclosingKey(node), this.isCallee(node) ? 'call' : 'reference');
      if (binding.kind === 'global' && RUNTIME_GLOBALS.has(object.text)) return;
      // `cart.add()` with `const cart = useCart()` is recorded with the identifier; `save.cancel()` with
      // `const { save } = useEditor()` reaches a member of what `save` holds, looked up by name below.
      const result = binding.kind === 'local' || binding.kind === 'own' ? this.resultOf(object, binding.declaration) : null;
      if (result && result.callee !== object) return;
    }
    // `useCart().add()`: a member of what a function of this repository returns.
    const call = unwrapExpression(object);
    if (ts.isCallExpression(call) && ts.isIdentifier(call.expression)) {
      const producer = this.bindingOf(call.expression, call.expression.text);
      if (producer.kind === 'import' || producer.kind === 'own') return this.recordResult({ producer, property: name, callee: node }, this.line(node.name), this.enclosingKey(node));
    }
    const line = this.line(node.name);
    const from = this.enclosingKey(node);
    // `rec: Recorder` then `rec.start()`: the declared class's method.
    if (ts.isIdentifier(object) && this.isReference(object)) {
      const binding = this.bindingOf(object, object.text);
      // `Registry.global()` or `actions.error()` written where that class or object is declared.
      if (binding.kind === 'own' && this.facts.declarations.has(`${binding.key}.${name}`)) return this.record({ kind: 'own', key: `${binding.key}.${name}` }, name, null, line, from, this.isCallee(node) ? 'call' : 'reference');
      const type = classTypeOf(binding.declaration?.type);
      if ((binding.kind === 'local' || binding.kind === 'own') && type) {
        const typeBinding = this.bindingOf(type.typeName, type.typeName.text);
        const kind = this.isCallee(node) ? 'call' : 'reference';
        if (typeBinding.kind === 'import') return this.facts.addReference(`m:${typeBinding.module}`, 'import', line, from, kind, typeBinding.name, name);
        if (typeBinding.kind === 'own' && this.facts.declarations.has(`${typeBinding.key}.${name}`)) return this.record({ kind: 'own', key: `${typeBinding.key}.${name}` }, name, null, line, from, kind);
      }
    }
    const initializer = ts.isIdentifier(object) ? this.constInitializer(object) : null;
    const required = initializer ? this.requiredModule(initializer) : undefined;
    if (required === null) return; // a package's member
    if (required) return this.facts.addReference(`m:${required}`, 'import', line, from, this.isCallee(node) ? 'call' : 'reference', '*', name);
    const handed = initializer ? this.methodHandedTo(initializer, name) : null;
    if (!handed && initializer && this.isForeignValue(initializer)) return;
    const own = handed ? { kind: 'own', key: handed } : object.kind === ts.SyntaxKind.ThisKeyword ? this.thisMember(node, name) : null;
    const kind = this.isCallee(node) ? 'call' : 'reference';
    if (own) this.record(own, name, null, line, from, kind);
    else if (kind === 'call' || this.handsOn(node)) this.facts.addReference(`n:${name}`, 'member', line, from, kind);
  }

  /**
   * Whether a member read gives the value to other code (`run(api.save)`, `{ onSave: api.save }`,
   * `const save = api.save`): only then may it be a function called later. `if (x.error)` or `a.global || b`
   * only reads a value.
   */
  handsOn(node) {
    let current = node;
    while (current.parent && (ts.isParenthesizedExpression(current.parent) || ts.isAsExpression(current.parent) || ts.isNonNullExpression(current.parent))) current = current.parent;
    const parent = current.parent;
    if (!parent) return false;
    if (ts.isCallExpression(parent) || ts.isNewExpression(parent)) return parent.arguments?.includes(current) ?? false;
    if (ts.isBinaryExpression(parent)) return parent.operatorToken.kind === ts.SyntaxKind.EqualsToken && parent.right === current;
    return ts.isVariableDeclaration(parent) || ts.isPropertyAssignment(parent) || ts.isReturnStatement(parent) || ts.isArrayLiteralExpression(parent) || ts.isJsxExpression(parent) || ts.isArrowFunction(parent) || ts.isSpreadElement(parent) || ts.isExportAssignment(parent);
  }

  /** The value a `const` was created with, so `x.name()` can be read against what `x` is. */
  constInitializer(identifier) {
    for (let current = identifier.parent; current; current = current.parent) {
      const declaration = this.scopeBindings(current).get(identifier.text);
      if (!declaration) continue;
      if (!ts.isVariableDeclaration(declaration) || !ts.isIdentifier(declaration.name) || !(declaration.parent.flags & ts.NodeFlags.Const)) return null;
      let value = declaration.initializer;
      while (value && (ts.isParenthesizedExpression(value) || ts.isAwaitExpression(value) || ts.isAsExpression(value) || ts.isNonNullExpression(value) || ts.isSatisfiesExpression(value))) value = value.expression;
      return value ?? null;
    }
    return null;
  }

  /** `require('x')`: the repository module it loads, null for a package, undefined when it is no `require`. */
  requiredModule(value) {
    if (!ts.isCallExpression(value) || !ts.isIdentifier(value.expression) || value.expression.text !== 'require' || !value.arguments[0] || !ts.isStringLiteralLike(value.arguments[0])) return undefined;
    if (this.bindingOf(value.expression, 'require').kind !== 'global') return undefined;
    const specifier = value.arguments[0].text;
    const { module } = resolveSpecifier(this.facts.path, specifier);
    if (module) return module;
    return specifier.startsWith('.') || specifier.startsWith('/') ? undefined : null;
  }

  /**
   * A value whose methods cannot be this repository's functions: a literal, or an instance of a class that is
   * neither declared nor imported here (`new Map()`, `new URLSearchParams()`, a package class).
   */
  isForeignValue(value) {
    if (ts.isArrayLiteralExpression(value) || ts.isStringLiteralLike(value) || ts.isTemplateExpression(value) || ts.isNumericLiteral(value) || ts.isRegularExpressionLiteral(value)) return true;
    if (ts.isNewExpression(value) && ts.isIdentifier(value.expression)) {
      const kind = this.bindingOf(value.expression, value.expression.text).kind;
      return kind === 'global' || kind === 'package';
    }
    return false;
  }

  /** `const state = reactive({ reset() {} }); state.reset()`: the method written in the object handed to that call. */
  methodHandedTo(value, name) {
    if (!(ts.isCallExpression(value) || ts.isNewExpression(value))) return null;
    const objects = [...(value.arguments ?? [])].filter((argument) => ts.isObjectLiteralExpression(argument));
    while (objects.length) {
      for (const property of objects.shift().properties) {
        if (propertyName(property.name) === name && (ts.isMethodDeclaration(property) || (ts.isPropertyAssignment(property) && isFunctionValue(property.initializer)))) {
          return this.declarationKey(ts.isMethodDeclaration(property) ? property : property.initializer);
        }
        if (ts.isPropertyAssignment(property) && ts.isObjectLiteralExpression(property.initializer)) objects.push(property.initializer);
      }
    }
    return null;
  }

  /** `import('./x')`, `require('./x')` and Vite's `import.meta.glob(...)`: the module runs when that code runs. */
  visitLoadingCall(node) {
    const callee = node.expression;
    const argument = node.arguments[0];
    const from = this.enclosingKey(node);
    const isRequire = ts.isIdentifier(callee) && callee.text === 'require' && this.bindingOf(callee, 'require').kind === 'global';
    const loads = (module, dynamic) => this.facts.addReference(`m:${module}`, 'module', this.line(node), from, isRequire ? 'require' : dynamic ? 'lazy import' : 'import');
    if ((callee.kind === ts.SyntaxKind.ImportKeyword || isRequire) && argument && ts.isStringLiteralLike(argument)) {
      const { module, bases } = resolveSpecifier(this.facts.path, argument.text);
      for (const base of bases) this.facts.importBases.add(base);
      if (module) loads(module, !isRequire);
      return;
    }
    const isGlob = ts.isPropertyAccessExpression(callee) && callee.name.text === 'glob' && ts.isMetaProperty(callee.expression);
    if (!isGlob || !argument) return;
    const glob = { positive: [], negative: [] };
    for (const pattern of ts.isArrayLiteralExpression(argument) ? argument.elements : [argument]) {
      if (!ts.isStringLiteralLike(pattern)) {
        this.facts.warnings.push(`${this.facts.path}:${this.line(node)}: import.meta.glob with a computed pattern; the modules it loads are unknown`);
        return;
      }
      const negated = pattern.text.startsWith('!');
      const text = negated ? pattern.text.slice(1) : pattern.text;
      const bases = text.startsWith('.') ? [toRepoPath(join(repoRoot, dirname(this.facts.path), text))].filter(Boolean) : specifierCandidates(this.facts.path, text);
      glob[negated ? 'negative' : 'positive'].push(...bases.map(globToRegExp));
    }
    this.facts.globs.push(glob);
    for (const path of known) {
      if (glob.positive.some((regex) => regex.test(path)) && !glob.negative.some((regex) => regex.test(path))) loads(path, true);
    }
  }

  visitDeclaration(node, source) {
    this.source = source;
    this.visitExport(node);
    const isVariable = isTopLevelVariable(node) && !isFunctionValue(node.initializer);
    if (!isVariable && (!(ts.isFunctionLike(node) || ts.isClassLike(node)) || ts.isPartOfTypeNode(node) || (!node.body && !ts.isClassLike(node)))) return;
    const key = this.declarationKey(node);
    if (key === null) return;
    // A wrapped function (`const save = useCallback(...)`) starts where its variable does.
    const range = { line: this.line(wrappedIn(node) ? node.parent.parent : node), end: source.getLineAndCharacterOfPosition(node.getEnd()).line + 1 };
    const ranges = this.facts.declarations.get(key);
    if (ranges) ranges.push(range);
    else this.facts.declarations.set(key, [range]);
    if (this.facts.isComponent && this.isComponentHook(node)) this.facts.componentHooks.add(key);
    if (isVariable) this.facts.loadTimeKeys.add(key);
    if (ts.isClassLike(node)) this.recordBase(node, key);
    if (!this.isMember(node)) return;
    const receiver = ts.isClassLike(node.parent) || ts.isPropertyDeclaration(node.parent) ? null : this.objectReceiver(node);
    if (assignedToMember(node)) return this.assignedMember(node, key);
    if (!receiver) return this.facts.memberKeys.add(key);
    let holder = receiver.parent;
    while (holder && ts.isParenthesizedExpression(holder)) holder = holder.parent;
    if (ts.isReturnStatement(holder) || ts.isArrowFunction(holder)) {
      // Returned by a named function: reached through its result. Returned by an anonymous one
      // (`useImperativeHandle(ref, () => ({ open() {} }))`), the object goes where no name follows it: by name.
      let producer = ts.isArrowFunction(holder) ? holder : holder.parent;
      while (producer && !ts.isFunctionLike(producer)) producer = producer.parent;
      if (!producer || this.declarationKey(producer) === null) this.facts.memberKeys.add(key);
      return;
    }
    this.facts.handedOver.set(key, { from: this.enclosingKey(receiver), line: this.line(receiver) });
    // `export const useCart = defineStore(..., { actions: { add() {} } })`: `useCart().add()` may reach it.
    const producer = this.enclosingKey(receiver);
    if (producer !== this.facts.topSymbol && !producer.includes('.')) this.returnMember(producer, key.slice(key.lastIndexOf('.') + 1), key, false);
  }

  recordBase(node, key) {
    const base = node.heritageClauses?.find((clause) => clause.token === ts.SyntaxKind.ExtendsKeyword)?.types[0]?.expression;
    if (!base || !ts.isIdentifier(base)) return;
    const binding = this.bindingOf(base, base.text);
    if (binding.kind === 'own') this.facts.classBases.set(key, { key: binding.key });
    else if (binding.kind === 'import') this.facts.classBases.set(key, { module: binding.module, name: binding.name });
  }

  /**
   * `debounced.cancel = () => ...`: found by name, and through the result of the function that returns
   * `debounced` (`const save = debounce(...); save.cancel()`).
   */
  assignedMember(node, key) {
    this.facts.memberKeys.add(key);
    const target = node.parent.left;
    if (!ts.isIdentifier(target.expression)) return;
    let producer = node.parent.parent;
    while (producer && !ts.isFunctionLike(producer)) producer = producer.parent;
    const producerKey = producer?.body ? this.declarationKey(producer) : null;
    if (producerKey !== null && returnsName(producer.body, target.expression.text)) this.returnMember(producerKey, target.name.text, key, true);
  }

  returnMember(functionKey, property, key, sure) {
    const members = this.facts.returnedMembers.get(functionKey);
    if (members) members.push([property, key, sure]);
    else this.facts.returnedMembers.set(functionKey, [[property, key, sure]]);
  }

  /** `return { save, apply: applyLabels, load() {} }`: which of the function's own functions each property is. */
  visitReturnedObject(node) {
    const object = unwrapExpression(ts.isReturnStatement(node) ? node.expression : node.body);
    if (!object || !ts.isObjectLiteralExpression(object)) return;
    const functionKey = ts.isReturnStatement(node) ? this.enclosingKey(node) : this.declarationKey(node);
    if (!functionKey || functionKey === this.facts.topSymbol) return;
    for (const property of object.properties) {
      const name = propertyName(property.name);
      if (name === null) continue;
      let key = null;
      if (ts.isMethodDeclaration(property)) key = this.declarationKey(property);
      else if (ts.isShorthandPropertyAssignment(property)) key = this.ownKey(property.name);
      else if (ts.isPropertyAssignment(property)) key = ts.isIdentifier(property.initializer) ? this.ownKey(property.initializer) : isFunctionValue(property.initializer) ? this.declarationKey(property.initializer) : null;
      if (key) this.returnMember(functionKey, name, key, true);
    }
  }

  ownKey(identifier) {
    const binding = this.bindingOf(identifier, identifier.text);
    return binding.kind === 'own' ? binding.key : null;
  }

  /**
   * A const made from a call of a function of this repository: `const { save: store } = useLabels()` or
   * `const cart = useCart()`. Returns that function's binding and the property of its result a use reaches.
   */
  resultOf(identifier, declaration) {
    if (!declaration || !ts.isVariableDeclaration(declaration) || !(declaration.parent.flags & ts.NodeFlags.Const)) return null;
    const call = unwrapExpression(declaration.initializer);
    if (!call || !ts.isCallExpression(call) || !ts.isIdentifier(call.expression)) return null;
    const producer = this.bindingOf(call.expression, call.expression.text);
    if (producer.kind !== 'import' && producer.kind !== 'own') return null;
    if (ts.isIdentifier(declaration.name)) {
      const property = this.memberAfter(identifier);
      return property ? { producer, property, callee: identifier.parent } : null;
    }
    if (!ts.isObjectBindingPattern(declaration.name)) return null;
    const element = declaration.name.elements.find((candidate) => ts.isIdentifier(candidate.name) && candidate.name.text === identifier.text);
    if (!element || element.dotDotDotToken) return null;
    return { producer, property: element.propertyName ? propertyName(element.propertyName) : element.name.text, callee: identifier };
  }

  recordResult(result, line, from, heldAs = null) {
    const kind = this.isCallee(result.callee) ? 'call' : 'reference';
    const { producer, property } = result;
    if (producer.kind === 'import') this.facts.addReference(`m:${producer.module}`, 'result', line, from, kind, producer.name, property, heldAs);
    else this.facts.addReference(`o:${producer.key}`, 'result', line, from, kind, producer.key, property, heldAs);
  }

  /**
   * Stored as `{ mapResponse: format }` or passed as `onCreated={reload}`: the function is later called as
   * `field.mapResponse(...)` or `onCreated()`, so callers look that name up too.
   */
  heldAs(node) {
    const parent = node.parent;
    if (ts.isPropertyAssignment(parent) && parent.initializer === node) return propertyName(parent.name);
    if (ts.isShorthandPropertyAssignment(parent)) return node.text;
    if (!ts.isJsxExpression(parent) || !ts.isJsxAttribute(parent.parent)) return null;
    // A prop is called by that component only: `onCreated\n<its file>`. A DOM element's or a package
    // component's handler is called by the browser or the package.
    const name = propertyName(parent.parent.name);
    const tag = parent.parent.parent.parent.tagName;
    if (!ts.isIdentifier(tag)) return name;
    if (/^[a-z]/.test(tag.text)) return null;
    const component = this.bindingOf(tag, tag.text);
    if (component.kind === 'package') return null;
    const file = component.kind === 'import' ? component.module : component.kind === 'own' ? this.facts.path : null;
    return file ? `${name}\n${file}` : name;
  }

  /** An identifier written as a property of an object the function returns: it hands the function out. */
  isReturnedValue(node) {
    return this.objectHolder(node) === 'returned';
  }

  /**
   * An identifier written as a property of an object literal that is returned ('returned') or given to a call,
   * a constructor or a JSX prop ('handed'): `return { save }`, `provide(KEY, { save })`, `value={{ save }}`.
   * The function then goes where that object goes; where it is called is found by its property name.
   */
  objectHolder(node) {
    const parent = node.parent;
    const property = ts.isShorthandPropertyAssignment(parent) || (ts.isPropertyAssignment(parent) && parent.initializer === node) ? parent : null;
    let holder = property?.parent?.parent;
    while (holder && ts.isParenthesizedExpression(holder)) holder = holder.parent;
    if (!property || !holder) return null;
    if (ts.isCallExpression(holder) && ts.isIdentifier(holder.expression) && holder.expression.text === 'defineExpose') return null; // see visitExpose
    if (ts.isReturnStatement(holder) || ts.isArrowFunction(holder)) return 'returned';
    if (ts.isCallExpression(holder) || ts.isNewExpression(holder) || ts.isJsxExpression(holder)) return 'handed';
    return null;
  }

  /** `{ save: store }` destructured from anything: the property a local stands for. */
  destructuredProperty(declaration, name) {
    const pattern = declaration && (ts.isVariableDeclaration(declaration) || ts.isParameter(declaration)) ? declaration.name : null;
    if (!pattern || !ts.isObjectBindingPattern(pattern)) return null;
    const element = pattern.elements.find((candidate) => ts.isIdentifier(candidate.name) && candidate.name.text === name);
    if (!element || element.dotDotDotToken) return null;
    return element.propertyName ? propertyName(element.propertyName) : element.name.text;
  }

  isMember(node) {
    const parent = node.parent;
    if (isFunctionValue(node) && assignedToMember(node)) return true;
    return ts.isMethodDeclaration(node) || ts.isGetAccessorDeclaration(node) || ts.isSetAccessorDeclaration(node) || ts.isConstructorDeclaration(node) || (isFunctionValue(node) && (ts.isPropertyAssignment(parent) || ts.isPropertyDeclaration(parent)));
  }

  /**
   * The outermost object literal holding a method, when that object is handed to other code (a call argument,
   * a return value, an assignment) rather than named: the module-level `const api = {...}` and a component's
   * `export default {...}` are reached by member name instead.
   */
  objectReceiver(node) {
    let object = ts.isMethodDeclaration(node) || ts.isGetAccessorDeclaration(node) || ts.isSetAccessorDeclaration(node) ? node.parent : node.parent.parent;
    while (object.parent && (ts.isPropertyAssignment(object.parent) || ts.isArrayLiteralExpression(object.parent)) && (ts.isObjectLiteralExpression(object.parent.parent) || ts.isArrayLiteralExpression(object.parent))) {
      object = ts.isArrayLiteralExpression(object.parent) ? object.parent : object.parent.parent;
    }
    const holder = object.parent;
    if (ts.isExportAssignment(holder) || ((ts.isCallExpression(holder) || ts.isNewExpression(holder)) && ts.isExportAssignment(holder.parent))) return null;
    if (isTopLevelVariable(holder)) return null;
    return object;
  }

  /** A function in the component's options outside `methods` is called by Vue itself (lifecycle, computed, watch). */
  isComponentHook(node) {
    let property = ts.isMethodDeclaration(node) || ts.isGetAccessorDeclaration(node) ? node : node.parent;
    if (!(ts.isMethodDeclaration(property) || ts.isPropertyAssignment(property) || ts.isGetAccessorDeclaration(property))) return false;
    let object = property.parent;
    for (let depth = 0; depth < 2 && object && ts.isObjectLiteralExpression(object); depth++) {
      const holder = object.parent;
      if (ts.isExportAssignment(holder) || (ts.isCallExpression(holder) && ts.isExportAssignment(holder.parent))) return depth === 0 || propertyName(property.name) !== 'methods';
      if (!ts.isPropertyAssignment(holder)) return false;
      property = holder;
      object = holder.parent;
    }
    return false;
  }

  visitExport(node) {
    const facts = this.facts;
    if (ts.isExportDeclaration(node)) {
      if (node.isTypeOnly) return;
      let module = null;
      if (node.moduleSpecifier && ts.isStringLiteral(node.moduleSpecifier)) {
        const resolved = resolveSpecifier(facts.path, node.moduleSpecifier.text);
        for (const base of resolved.bases) facts.importBases.add(base);
        if (!resolved.module) return;
        module = resolved.module;
        facts.addReference(`m:${module}`, 'module', this.line(node), facts.topSymbol, 'import');
      }
      const clause = node.exportClause;
      if (!clause) return module && facts.starExports.push(module);
      if (ts.isNamespaceExport(clause)) return facts.exports.set(clause.name.text, { module, name: '*' });
      for (const element of clause.elements) {
        const local = (element.propertyName ?? element.name).text;
        facts.exports.set(element.name.text, module ? { module, name: local } : this.exportOfLocal(local, node));
      }
      return;
    }
    if (ts.isExportAssignment(node) && !node.isExportEquals) {
      const wrapped = ts.isCallExpression(node.expression) && node.expression.arguments.length === 1 && ts.isIdentifier(node.expression.arguments[0]) ? node.expression.arguments[0] : null;
      if (ts.isIdentifier(node.expression)) facts.exports.set('default', this.exportOfLocal(node.expression.text, node));
      // `export default memo(Item)`, `forwardRef(Item)`: the export renders that component.
      else if (wrapped) facts.exports.set('default', this.exportOfLocal(wrapped.text, node));
      else facts.exports.set('default', { local: isFunctionValue(node.expression) ? 'default' : facts.topSymbol });
      const object = defaultExportObject(node.expression);
      for (const property of object?.properties ?? []) {
        const name = propertyName(property.name);
        if (name === null) continue;
        if (ts.isShorthandPropertyAssignment(property)) facts.defaultMembers.set(name, this.exportOfLocal(name, node));
        else if (ts.isMethodDeclaration(property) || (ts.isPropertyAssignment(property) && isFunctionValue(property.initializer))) facts.defaultMembers.set(name, { local: name });
        else if (ts.isPropertyAssignment(property) && ts.isIdentifier(property.initializer)) facts.defaultMembers.set(name, this.exportOfLocal(property.initializer.text, node));
      }
      return;
    }
    if (!node.parent || !ts.isSourceFile(node.parent) || !hasModifier(node, ts.SyntaxKind.ExportKeyword)) return;
    if (ts.isFunctionDeclaration(node) || ts.isClassDeclaration(node)) {
      const isDefault = hasModifier(node, ts.SyntaxKind.DefaultKeyword);
      if (node.name || isDefault) facts.exports.set(isDefault ? 'default' : node.name.text, { local: node.name?.text ?? 'default' });
    } else if (ts.isVariableStatement(node)) {
      for (const declaration of node.declarationList.declarations) for (const name of bindingNames(declaration.name)) facts.exports.set(name.text, { local: name.text });
    }
  }

  /** An exported local is either declared here or itself imported (a re-export spelled in two statements). */
  exportOfLocal(local, node) {
    const binding = this.bindingOf(node, local);
    return binding.kind === 'import' ? { module: binding.module, name: binding.name } : { local: binding.kind === 'own' ? binding.key : local };
  }

  /** Expressions and component tags of a Vue template; `v-for` and slot variables are template locals. */
  visitTemplate(node, locals) {
    let innerLocals = locals;
    const props = node.props ?? [];
    for (const prop of props) {
      if (prop.type !== 7 || !prop.exp || (prop.name !== 'for' && prop.name !== 'slot')) continue;
      innerLocals = new Set(innerLocals);
      const parsed = prop.forParseResult;
      const pattern = prop.name === 'slot' ? prop.exp.content : parsed ? [parsed.value, parsed.key, parsed.index].map((part) => part?.content ?? '_').join(',') : prop.exp.content.split(/\s+(?:in|of)\s+/)[0].replace(/^\(|\)$/g, '');
      const parameters = ts.createSourceFile('locals.ts', `((${pattern}) => 0)`, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
      const fromSlot = prop.name === 'slot';
      (function collect(child) {
        // A slot prop (`v-slot="{ scrollTo }"`) is a function the child component hands out: `slot:` marks it.
        if (ts.isParameter(child)) for (const name of bindingNames(child.name)) innerLocals.add(name.text).add(fromSlot ? `slot:${name.text}` : name.text);
        ts.forEachChild(child, collect);
      })(parameters);
    }
    if (node.type === 1 && (node.tagType === 1 || /[A-Z-]/.test(node.tag))) this.visitComponentTag(node, innerLocals);
    for (const prop of props) {
      if (prop.type !== 7 || !prop.exp || prop.name === 'slot') continue;
      if (prop.name === 'for') this.visitTemplateExpression(prop.forParseResult?.source?.content ?? prop.exp.content.split(/\s+(?:in|of)\s+/).slice(1).join(' in '), this.lineOfOffset(prop.exp.loc.start.offset), locals);
      else this.visitTemplateExpression(prop.exp.content, this.lineOfOffset(prop.exp.loc.start.offset), innerLocals);
    }
    if (node.type === 5 && node.content) this.visitTemplateExpression(node.content.content, this.lineOfOffset(node.content.loc.start.offset), innerLocals);
    for (const child of node.children ?? []) this.visitTemplate(child, innerLocals);
  }

  /** What a template name means: an import or binding of `<script setup>`, an option of the component, or a global. */
  templateBinding(name) {
    const moduleBinding = this.moduleBindings.get(name);
    if (moduleBinding) return moduleBinding;
    if (this.facts.declarations.has(name)) return { kind: 'own', key: name };
    if (this.setupSource && this.scopeBindings(this.setupSource).has(name)) return this.bindingOf(this.setupSource.statements[0] ?? this.setupSource, name);
    return { kind: 'global' };
  }

  componentBinding(name) {
    const binding = this.templateBinding(name);
    const autoImported = binding.kind === 'global' ? resolution.autoComponents[name] : undefined;
    return autoImported && known.has(autoImported) ? { kind: 'import', module: autoImported, name: 'default' } : binding;
  }

  visitComponentTag(node, locals) {
    const [name, member = null] = node.tag.replace(/(^|-)(\w)/g, (_, __, letter) => letter.toUpperCase()).split('.');
    if (locals.has(name) || locals.has(node.tag)) return;
    let binding = this.templateBinding(node.tag);
    if (binding.kind === 'global') binding = this.componentBinding(name);
    if (binding.kind === 'global' && node.tagType !== 1) return;
    this.record(binding, member ? `${name}.${member}` : name, member, this.lineOfOffset(node.loc.start.offset), 'default', 'component');
  }

  visitTemplateExpression(code, startLine, locals) {
    let source = ts.createSourceFile('template.ts', `(${code}\n)`, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    if (source.parseDiagnostics?.length) source = ts.createSourceFile('template.ts', code, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
    const lineOf = (node) => startLine + source.getLineAndCharacterOfPosition(node.getStart(source)).line;
    const boundInExpression = (node, name) => {
      for (let current = node.parent; current; current = current.parent) {
        if (ts.isFunctionLike(current) && current.parameters.some((parameter) => bindingNames(parameter.name).some((identifier) => identifier.text === name))) return true;
      }
      return false;
    };
    const visit = (node) => {
      if (ts.isIdentifier(node) && locals.has(`slot:${node.text}`) && ts.isCallExpression(node.parent) && node.parent.expression === node) {
        this.facts.addReference(`n:${node.text}`, 'member', lineOf(node), 'default', 'call');
      } else if (ts.isIdentifier(node) && this.isReference(node) && !locals.has(node.text) && !boundInExpression(node, node.text)) {
        const binding = this.templateBinding(node.text);
        const called = ts.isCallExpression(node.parent) && node.parent.expression === node;
        // `@click="toggleSidebar"` hands it to the template as a handler.
        const property = binding.kind === 'local' ? this.destructuredProperty(binding.declaration, node.text) : null;
        if (property) this.facts.addReference(`n:${property}`, 'member', lineOf(node), 'default', called ? 'call' : 'passed');
        this.record(binding, node.text, binding.kind === 'import' ? this.memberAfter(node) : null, lineOf(node), 'default', 'template');
      } else if (ts.isPropertyAccessExpression(node)) {
        const objectKind = ts.isIdentifier(node.expression) && !locals.has(node.expression.text) ? this.templateBinding(node.expression.text).kind : null;
        // `sliderRef?.scrollNext()`: a method a child component exposes, called through its template ref.
        const called = ts.isCallExpression(node.parent) && node.parent.expression === node;
        if (objectKind !== 'import' && objectKind !== 'package') this.facts.addReference(`n:${node.name.text}`, 'member', lineOf(node.name), 'default', called ? 'call' : 'template');
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
}

function readFileFacts(path) {
  const facts = new FileFacts(path);
  facts.stamp = fileStamp(path);
  let text;
  try {
    text = readFileSync(join(repoRoot, path), 'utf8');
  } catch {
    return null;
  }
  try {
    new FileReader(facts, text).read();
    return facts;
  } catch (error) {
    const empty = new FileFacts(path);
    empty.stamp = facts.stamp;
    empty.warnings.push(`${path} could not be analysed (${error.message}); references inside it are unknown`);
    return empty;
  }
}

/** Facts of every script file, kept until the core reports that file changed. */
class FactStore {
  constructor() {
    this.files = new Map();
    /** reference index key => files holding references under it */
    this.filesByKey = new Map();
    this.filesByBase = new Map();
    this.globFiles = new Set();
    this.filesByDeclaration = new Map();
    this.pending = new Set(known);
    /** bumped when a file is re-read here, so a warm-up reader's older answer for it is dropped */
    this.versions = new Map();
    /** bumped when a file appears or vanishes: module resolution of every file read before may differ */
    this.knownSerial = 0;
    this.readers = new Set();
    this.waiting = [];
    /** what each answered `callers` read: index keys and files; the core keeps the answer until one of them changes */
    this.answerDependencies = new Map();
    this.answersByDependency = new Map();
    this.recording = null;
    this.invalidated = new Set();
    /** facts read since the saved copy was written */
    this.unsaved = false;
    this.savedOnce = false;
  }

  // Facts saved between sessions: a new server reads them back instead of parsing every file again. A file whose
  // (mtime, size) changed is read again; a file that appeared or vanished makes its importers read again, as
  // `sync` does. Saved facts depend on this driver, Node and the module resolution: any change there reads all.
  static savedPath() {
    const cache = process.env.SCOUT_CACHE || join(homedir(), '.cache', 'scout');
    return join(cache, createHash('sha1').update(repoRoot).digest('hex').slice(0, 16), 'javascript-facts');
  }

  static savedKey() {
    return JSON.stringify([createHash('sha1').update(readFileSync(driverPath)).digest('hex'), process.version, ts?.version, resolution.portable]);
  }

  /** The saved facts when they were written for this driver and this module resolution, else null. */
  static readSaved() {
    try {
      const saved = JSON.parse(gunzipSync(readFileSync(FactStore.savedPath())).toString('utf8'));
      return saved.key === FactStore.savedKey() ? saved : null;
    } catch {
      return null; // none yet, or unreadable: the files are read
    }
  }

  loadSaved(saved) {
    if (!saved) return;
    for (const json of saved.files) {
      const current = known.has(json.path) && json.stamp !== null && JSON.stringify(json.stamp) === JSON.stringify(fileStamp(json.path));
      if (current) this.install(json.path, FileFacts.fromJSON(json));
    }
    const before = new Set(saved.known);
    const reshaped = [...before].filter((path) => !known.has(path)).concat([...known].filter((path) => !before.has(path)));
    for (const path of reshaped) {
      const importers = basesOf(path).flatMap((base) => [...(this.filesByBase.get(base) ?? [])]);
      const globbing = [...this.globFiles].filter((globFile) => this.files.get(globFile).globs.some((glob) => glob.positive.some((regex) => regex.test(path))));
      for (const stale of [...importers, ...globbing]) {
        this.forget(stale);
        this.pending.add(stale);
      }
    }
    this.unsaved = this.pending.size > 0;
    this.savedOnce = !this.unsaved;
  }

  save() {
    if (!this.unsaved || this.pending.size > 0) return;
    const path = FactStore.savedPath();
    try {
      mkdirSync(dirname(path), { recursive: true });
      const files = [...this.files.values()].map((facts) => facts.toJSON());
      const bundles = skippedBundles.map((bundle) => [bundle, fileStamp(bundle)]);
      writeFileSync(`${path}.tmp`, gzipSync(JSON.stringify({ key: FactStore.savedKey(), known: [...known], files, bundles }), { level: 1 }));
      renameSync(`${path}.tmp`, path);
      this.unsaved = false;
      this.savedOnce = true;
    } catch {
      // a full disk only costs the next server a slower start
    }
  }

  using(indexKey) {
    this.recording?.add(indexKey);
    return this.filesByKey.get(indexKey);
  }

  /** Answers that read this index key or this file are out of date; the next sync tells the core which. */
  invalidate(dependency) {
    for (const symbol of this.answersByDependency.get(dependency) ?? []) {
      this.invalidated.add(symbol);
      for (const other of this.answerDependencies.get(symbol) ?? []) this.answersByDependency.get(other)?.delete(symbol);
      this.answerDependencies.delete(symbol);
    }
    this.answersByDependency.delete(dependency);
  }

  remember(symbol, dependencies) {
    this.answerDependencies.set(symbol, dependencies);
    for (const dependency of dependencies) FactStore.add(this.answersByDependency, dependency, symbol);
  }

  static add(map, key, path) {
    const set = map.get(key);
    if (set) set.add(path);
    else map.set(key, new Set([path]));
  }

  static remove(map, key, path) {
    const set = map.get(key);
    if (set && set.delete(path) && set.size === 0) map.delete(key);
  }

  indexKeys(facts) {
    return [...facts.references.keys()].filter((key) => !key.startsWith('o:')).concat(facts.reexportedModules().map((module) => `m:${module}`));
  }

  install(path, facts) {
    this.pending.delete(path);
    for (const key of this.files.has(path) ? this.indexKeys(this.files.get(path)) : []) this.invalidate(key);
    for (const key of facts ? this.indexKeys(facts) : []) this.invalidate(key);
    this.invalidate(`f:${path}`);
    this.forget(path);
    if (!facts) return null;
    this.files.set(path, facts);
    for (const key of this.indexKeys(facts)) FactStore.add(this.filesByKey, key, path);
    for (const base of facts.importBases) FactStore.add(this.filesByBase, base, path);
    for (const key of facts.declarations.keys()) FactStore.add(this.filesByDeclaration, key, path);
    if (facts.globs.length) this.globFiles.add(path);
    return facts;
  }

  read(path) {
    this.unsaved = true;
    this.versions.set(path, (this.versions.get(path) ?? 0) + 1);
    return this.install(path, known.has(path) ? readFileFacts(path) : null);
  }

  forget(path) {
    const facts = this.files.get(path);
    if (!facts) return;
    this.files.delete(path);
    for (const key of this.indexKeys(facts)) FactStore.remove(this.filesByKey, key, path);
    for (const base of facts.importBases) FactStore.remove(this.filesByBase, base, path);
    for (const key of facts.declarations.keys()) FactStore.remove(this.filesByDeclaration, key, path);
    this.globFiles.delete(path);
  }

  get(path) {
    this.recording?.add(`f:${path}`);
    return this.files.get(path) ?? (this.pending.has(path) ? this.read(path) : null);
  }

  /**
   * Reads every file in a few low-priority child processes: parse trees are large and short-lived, and memory a
   * child used goes back to the system when it exits. What a child could not finish is read here afterwards.
   */
  startWarmUp(startedAt) {
    const files = [...this.pending];
    if (!files.length) return this.finishWarmUp(startedAt, 0);
    // Each reader holds the parser (about 250 MB at its peak): two keep the warm-up a few seconds without doubling that.
    const count = Math.max(1, Math.min(2, availableParallelism() - 1, Math.ceil(files.length / 200)));
    for (let slot = 0; slot < count; slot++) {
      const slice = files.filter((_, index) => index % count === slot);
      const versions = new Map(slice.map((path) => [path, this.versions.get(path) ?? 0]));
      const knownSerial = this.knownSerial;
      const child = spawn(process.execPath, [driverPath, '--read', repoRoot], { stdio: ['pipe', 'pipe', 'inherit'] });
      try {
        setPriority(child.pid, 19);
      } catch {
        // Lowering priority is a courtesy; some systems refuse it.
      }
      this.readers.add(child);
      createInterface({ input: child.stdout }).on('line', (line) => {
        const json = JSON.parse(line);
        const current = this.pending.has(json.path) && versions.get(json.path) === (this.versions.get(json.path) ?? 0) && knownSerial === this.knownSerial;
        if (current) {
          this.install(json.path, FileFacts.fromJSON(json));
          this.unsaved = true;
        }
      });
      child.on('close', () => {
        this.readers.delete(child);
        if (this.readers.size === 0) this.finishWarmUp(startedAt, count);
      });
      child.stdin.end(JSON.stringify({ known: [...known], resolution: resolution.portable, files: slice }));
    }
  }

  finishWarmUp(startedAt, readers) {
    const leftOver = this.pending.size;
    this.readAll();
    // Once every file is read for the first time; later edits are saved when the server leaves.
    if (!this.savedOnce) this.save();
    profilePhase('warm-up: done', startedAt, { files: this.files.size, readers, readHereAfterwards: leftOver, residentMb: Math.round(process.memoryUsage().rss / 1048576) });
    for (const resolve of this.waiting.splice(0)) resolve();
  }

  readAll() {
    for (const path of [...this.pending]) this.read(path);
  }

  /** Answers that look across files need every file read; they wait for the warm-up rather than repeat its work. */
  ready() {
    if (this.readers.size === 0) {
      this.readAll();
      return null;
    }
    return new Promise((resolve) => this.waiting.push(resolve));
  }

  sync(changed) {
    const restart = changed.find((path) => restartInputs.has(path));
    if (restart) {
      process.stderr.write(`restarting: ${restart} changed\n`);
      process.exit(0);
    }
    const reread = new Set();
    let componentsAppearedOrVanished = false;
    for (const path of changed) {
      if (!isScriptFile(path)) continue;
      const exists = existsSync(join(repoRoot, path));
      const appearedOrVanished = exists !== known.has(path);
      if (exists) known.add(path);
      else known.delete(path);
      reread.add(path);
      if (!appearedOrVanished) continue;
      componentsAppearedOrVanished ||= path.endsWith('.vue');
      // What a specifier resolves to, and what a glob matches, depends on which files exist.
      this.knownSerial++;
      for (const base of basesOf(path)) for (const importer of this.filesByBase.get(base) ?? []) reread.add(importer);
      for (const globFile of this.globFiles) {
        if (this.files.get(globFile).globs.some((glob) => glob.positive.some((regex) => regex.test(path)))) reread.add(globFile);
      }
    }
    if (componentsAppearedOrVanished) this.refreshAutoComponents(reread);
    for (const path of reread) this.read(path);
    const invalidated = [...this.invalidated];
    this.invalidated.clear();
    return { watch: [...restartInputs], warnings: this.warnings(), invalidated };
  }

  /** A component file appeared or vanished: ask the bundler plugin again which file each tag name means. */
  refreshAutoComponents(reread) {
    const before = resolution.autoComponents;
    const fresh = readModuleResolution().autoComponents;
    for (const name of new Set([...Object.keys(before), ...Object.keys(fresh)])) {
      if (before[name] === fresh[name]) continue;
      for (const path of this.filesByKey.get(`n:${name}`) ?? []) reread.add(path);
      for (const path of this.filesByKey.get(`m:${before[name]}`) ?? []) reread.add(path);
    }
    resolution.autoComponents = fresh;
    resolution.portable.autoComponents = fresh;
  }

  warnings() {
    const warnings = [...resolution.warnings];
    for (const facts of this.files.values()) warnings.push(...facts.warnings);
    return warnings;
  }

  symbolRef(path, key) {
    const facts = this.get(path);
    const declaration = facts?.declarations.get(key)?.[0];
    const kind = facts?.isTest ? 'test' : key === '(module)' && resolution.entries.includes(path) ? 'entry' : null;
    return { symbol: `${path}#${key}`, file: path, line: declaration?.line ?? 1, ...(kind ? { kind } : {}) };
  }

  parseSymbol(symbol) {
    const at = symbol.lastIndexOf('#');
    if (at < 0) return null;
    const path = symbol.slice(0, at);
    return known.has(path) ? { path, key: symbol.slice(at + 1) } : null;
  }

  /** Every (module, exported name, member) a symbol can be imported as, re-exports followed. */
  exportAliases(path, key) {
    const facts = this.get(path);
    const aliases = [];
    const addAlias = (alias) => {
      if (!aliases.some((known) => known.module === alias.module && known.name === alias.name && known.member === alias.member)) aliases.push(alias);
    };
    const dot = key.lastIndexOf('.');
    const owner = dot > 0 ? key.slice(0, dot) : null;
    for (const [exported, value] of facts.exports) {
      if (value.local === key) addAlias({ module: path, name: exported, member: null });
      if (owner && value.local === owner) addAlias({ module: path, name: exported, member: key.slice(dot + 1) });
    }
    for (const [name, value] of facts.defaultMembers) if (value.local === key) addAlias({ module: path, name: 'default', member: name });
    if (key === 'default' && facts.isComponent) addAlias({ module: path, name: 'default', member: null });
    for (let index = 0; index < aliases.length; index++) {
      const alias = aliases[index];
      for (const reexporter of this.using(`m:${alias.module}`) ?? []) {
        const other = this.files.get(reexporter);
        for (const [exported, value] of other.exports) {
          if (value.module !== alias.module) continue;
          if (value.name === alias.name) addAlias({ module: reexporter, name: exported, member: alias.member });
          else if (value.name === '*' && alias.member === null) addAlias({ module: reexporter, name: exported, member: alias.name });
        }
        for (const [name, value] of other.defaultMembers) {
          if (value.module === alias.module && value.name === alias.name && alias.member === null) addAlias({ module: reexporter, name: 'default', member: name });
        }
        if (other.starExports.includes(alias.module) && alias.name !== 'default') addAlias({ module: reexporter, name: alias.name, member: alias.member });
      }
    }
    return aliases;
  }

  /** The declaration behind an export, re-exports followed. */
  resolveExport(module, name, seen = new Set()) {
    if (seen.has(`${module}#${name}`)) return null;
    seen.add(`${module}#${name}`);
    const facts = this.get(module);
    const value = facts?.exports.get(name);
    if (value?.local) return { path: module, key: value.local };
    if (value?.module) return this.resolveExport(value.module, value.name, seen);
    for (const star of name === 'default' ? [] : (facts?.starExports ?? [])) {
      const found = this.resolveExport(star, name, seen);
      if (found) return found;
    }
    return null;
  }

  ancestorClasses(path, classKey) {
    const ancestors = [];
    let current = { path, key: classKey };
    for (;;) {
      const base = this.get(current.path)?.classBases.get(current.key);
      const next = base?.module ? this.resolveExport(base.module, base.name) : base ? { path: current.path, key: base.key } : null;
      if (!next || ancestors.some((known) => known.path === next.path && known.key === next.key)) return ancestors;
      ancestors.push(next);
      current = next;
    }
  }

  async callers(symbol) {
    const at = symbol.lastIndexOf('#');
    const target = this.parseSymbol(symbol);
    if (!target) {
      // A file that does not exist yet may appear; until then the answer is "not mine".
      if (at > 0) this.remember(symbol, [`f:${symbol.slice(0, at)}`]);
      return null;
    }
    await this.ready();
    this.recording = new Set();
    try {
      return this.callersOf(target);
    } finally {
      this.remember(symbol, this.recording);
      this.recording = null;
    }
  }

  callersOf(target) {
    const { path } = target;
    const facts = this.get(path);
    if (!facts) return [];
    // `file#default` of `export default function sub()` means `sub`.
    const exported = facts.declarations.has(target.key) ? null : facts.exports.get(target.key)?.local;
    const key = exported && facts.declarations.has(exported) ? exported : target.key;
    const edges = [];
    const edge = (filePath, reference, confidence) =>
      edges.push({ from: this.symbolRef(filePath, reference.from), at: { file: filePath, line: reference.line }, kind: reference.kind, confidence });

    // `lazy(() => import('./Orb'))` loads the default export: the component itself.
    const exportedAsDefault = key === 'default' || facts.exports.get('default')?.local === key;
    const loadEdges = (runsOnLoad) => {
      for (const importer of this.using(`m:${path}`) ?? []) {
        const importerFacts = this.files.get(importer);
        for (const reference of importerFacts.referencesUnder(`m:${path}`)) {
          if (reference.to !== 'module' || !(runsOnLoad || (exportedAsDefault && reference.kind === 'lazy import'))) continue;
          // A page an entry file loads through `import.meta.glob` is where the outside world reaches the code.
          if (reference.kind === 'lazy import' && importerFacts.globs.length && resolution.entries.includes(importer)) {
            edges.push({ from: { symbol: `${path} (loaded by ${importer}:${reference.line})`, file: path, line: 1, kind: 'entry' }, at: { file: importer, line: reference.line }, kind: 'page', confidence: 'exact' });
          } else edge(importer, reference, 'exact');
        }
      }
    };
    if (key === '(module)' || exportedAsDefault) loadEdges(key === '(module)');
    // Property names it was stored or passed under => the files a call by that name may reach it in (null: any).
    const propertyFiles = new Map();
    const holdsProperty = (property) => {
      if (!property) return;
      const [name, file] = property.split('\n');
      const files = propertyFiles.has(name) ? propertyFiles.get(name) : new Set();
      if (files !== null) propertyFiles.set(name, file ? files.add(file) : null);
    };
    if (facts.snippetKeys.has(key)) holdsProperty(key);
    // Returned in a literal by a named function: its callers are found through that function's result below.
    const returnedByName = [...facts.returnedMembers.values()].some((members) => members.some(([, memberKey]) => memberKey === key));
    for (const reference of facts.referencesUnder(`o:${key}`)) {
      if (reference.to === 'result') continue;
      // Handed out in an object: where it is called is found through the result, or by property name.
      edge(path, reference, reference.kind === 'returned' || reference.kind === 'handed' ? 'unresolved' : 'exact');
      if (!(reference.kind === 'returned' && returnedByName)) holdsProperty(reference.property);
    }
    // Reached through what a function returns: `const { save } = useLabels(); save()`.
    for (const [producer, members] of facts.returnedMembers) {
      for (const [property, memberKey, sure] of members) {
        if (memberKey !== key) continue;
        const confidence = sure ? 'exact' : 'likely';
        for (const reference of facts.referencesUnder(`o:${producer}`)) {
          if (reference.to !== 'result' || reference.member !== property) continue;
          edge(path, reference, confidence);
          holdsProperty(reference.property);
        }
        for (const alias of this.exportAliases(path, producer)) {
          if (alias.member !== null) continue;
          for (const importer of this.using(`m:${alias.module}`) ?? []) {
            for (const reference of this.files.get(importer).referencesUnder(`m:${alias.module}`)) {
              if (reference.to !== 'result' || reference.name !== alias.name || reference.member !== property) continue;
              edge(importer, reference, confidence);
              holdsProperty(reference.property);
            }
          }
        }
      }
    }
    // `this.area()` written in a base class runs this override when the object is of this class.
    const dot = key.lastIndexOf('.');
    if (dot > 0 && facts.classBases.has(key.slice(0, dot))) {
      for (const ancestor of this.ancestorClasses(path, key.slice(0, dot))) {
        for (const reference of this.get(ancestor.path)?.referencesUnder(`o:${ancestor.key}.${key.slice(dot + 1)}`) ?? []) edge(ancestor.path, reference, 'likely');
      }
    }
    const handedOver = facts.handedOver.get(key);
    // Given to other code that calls it back: who calls it is that code's business, so the hand-over is listed
    // for reading, not walked as a caller (calls through what the receiver returns are found above).
    if (handedOver) edges.push({ from: this.symbolRef(path, handedOver.from), at: { file: path, line: handedOver.line }, kind: 'callback', confidence: 'unresolved' });
    if (facts.componentHooks.has(key)) edges.push({ from: this.symbolRef(path, 'default'), at: { file: path, line: facts.declarations.get(key)[0].line }, kind: 'vue hook', confidence: 'exact' });

    const aliases = key === '(module)' ? [] : this.exportAliases(path, key);
    // A CommonJS module lists no exports to match: a member of what `require()` returned may be this function.
    if (facts.isCommonJs && key !== '(module)') {
      const member = key.slice(key.lastIndexOf('.') + 1);
      for (const importer of this.using(`m:${path}`) ?? []) {
        for (const reference of this.files.get(importer).referencesUnder(`m:${path}`)) {
          if (reference.to === 'import' && reference.name === '*' && reference.member === member) edge(importer, reference, 'likely');
        }
      }
    }
    for (const alias of aliases) {
      for (const importer of this.using(`m:${alias.module}`) ?? []) {
        for (const reference of this.files.get(importer).referencesUnder(`m:${alias.module}`)) {
          if (reference.to !== 'import') continue;
          const direct = reference.name === alias.name && (alias.member === null || reference.member === alias.member);
          const throughNamespace = reference.name === '*' && alias.member === null && reference.member === alias.name;
          if (direct || throughNamespace) {
            edge(importer, reference, reference.kind === 'returned' || reference.kind === 'handed' ? 'unresolved' : 'exact');
            holdsProperty(reference.property);
          }
        }
      }
    }

    // Names an unbound or untyped reference could mean this symbol by.
    const names = new Set(key === '(module)' ? [] : [key.slice(key.lastIndexOf('.') + 1)]);
    for (const alias of aliases) names.add(alias.member ?? alias.name);
    names.delete('default');
    const isComponent = key === 'default' && facts.isComponent;
    // A module loaded by `import()` or `require()` hands its exports over as members of an object.
    const loadedAsObject = [...(this.using(`m:${path}`) ?? [])].some((importer) => [...this.files.get(importer).referencesUnder(`m:${path}`)].some((reference) => reference.kind === 'lazy import' || reference.kind === 'require'));
    // A method of an object a factory returns (`createReader().read`) is called on whatever holds that object.
    const reachableAsMember = facts.memberKeys.has(key) || facts.isCommonJs || (loadedAsObject && aliases.length > 0) || returnedByName;
    // An unbound name means this symbol only when it is exported (a plugin may auto-import it) or lives in a
    // classic script, whose top level is global.
    const isClassicScript = !facts.isComponent && facts.exports.size === 0 && facts.starExports.length === 0 && !facts.importBases.size && !facts.isCommonJs;
    const reachableAsGlobal = !key.includes('.') && (aliases.length > 0 || isClassicScript);
    if (isComponent) names.add(basename(path).replace(/\.(vue|svelte)$/, '').replace(/(^|-)(\w)/g, (_, __, letter) => letter.toUpperCase()));
    for (const property of propertyFiles.keys()) names.add(property);
    // `const { save } = useLabels()`: found through the result when `useLabels` returns `save` in a literal.
    const followed = (reference, name) => {
      if (reference.name === null) return false;
      const producer = reference.member !== null ? this.resolveExport(reference.name, reference.member) : { path: reference.name, key: reference.property };
      return (this.get(producer?.path)?.returnedMembers.get(producer.key) ?? []).some(([property]) => property === name);
    };
    for (const name of names) {
      for (const filePath of this.using(`n:${name}`) ?? []) {
        for (const reference of this.files.get(filePath).referencesUnder(`n:${name}`)) {
          // Called through the property it was stored under: any `x.thatName(...)` may be it. Only calls: reading
          // `.settings` where a component sits is not using it.
          if (reference.to === 'member' && followed(reference, name)) continue;
          if (propertyFiles.has(name) && reference.to === 'member') {
            const files = propertyFiles.get(name);
            if ((reference.kind === 'call' || reference.kind === 'passed') && !isComponent && (files === null || files.has(filePath))) edge(filePath, reference, 'unresolved');
            continue;
          }
          // A function or variable reached as `object.name` was first stored in that object, and that store is
          // itself a reference listed above; only members, exposed functions and CommonJS exports are looked up by name.
          if (reference.to === 'member' && !(reachableAsMember && !isComponent)) continue;
          if (reference.to === 'global' && !reachableAsGlobal && !isComponent) continue;
          if (isComponent && reference.kind !== 'component') continue;
          edge(filePath, reference, 'unresolved');
        }
      }
    }
    // A value computed on load (`const variants = cva(...)`) is used where it is named; only one nothing names
    // is reached by loading its module.
    if (facts.loadTimeKeys.has(key) && !edges.length) loadEdges(true);
    return edges;
  }

  async resolve(query) {
    if (query.includes('::') || query.includes('\\')) return [];
    const direct = this.parseSymbol(query);
    if (direct) {
      const facts = this.get(direct.path);
      const exists = facts && (facts.declarations.has(direct.key) || facts.exports.has(direct.key) || direct.key === facts.topSymbol || direct.key === '(module)' || direct.key === 'default');
      return exists ? [this.symbolRef(direct.path, direct.key)] : [];
    }
    await this.ready();
    const found = [...(this.filesByDeclaration.get(query) ?? [])].map((path) => this.symbolRef(path, query));
    for (const path of known) if (/\.(vue|svelte)$/.test(path) && basename(path).replace(/\.(vue|svelte)$/, '') === query) found.push(this.symbolRef(path, 'default'));
    return found;
  }

  locate(path, line) {
    const facts = known.has(path) ? this.get(path) : null;
    if (!facts) return null;
    let best = null;
    for (const [key, ranges] of facts.declarations) {
      for (const range of ranges) if (range.line <= line && line <= range.end && (!best || range.line >= best.line)) best = { key, line: range.line };
    }
    return this.symbolRef(path, best ? best.key : facts.topSymbol);
  }
}

/** A warm-up reader: reads the files it is given and prints their facts, one JSON line each. */
async function serveReader() {
  let raw = '';
  for await (const chunk of process.stdin) raw += chunk;
  const input = JSON.parse(raw);
  resolution = compileResolution(input.resolution);
  known = new Set(input.known);
  for (const path of input.files) {
    const facts = readFileFacts(path);
    if (facts) process.stdout.write(JSON.stringify(facts) + '\n');
  }
}

let restartInputs;

function serveDriver() {
  const startedAt = performance.now();
  resolution = compileResolution(readModuleResolution());
  profilePhase('start: module resolution', startedAt);
  restartInputs = new Set(resolution.inputs);
  const saved = ts ? FactStore.readSaved() : null;
  known = new Set(listFiles(saved));
  profilePhase('start: files listed', startedAt, { files: known.size });
  const store = ts ? new FactStore() : null;
  store?.loadSaved(saved);
  profilePhase('start: saved facts read', startedAt, { files: store?.files.size });
  store?.startWarmUp(startedAt);

  const handlers = {
    probe: () => ({
      driver: 'javascript',
      typescript: ts?.version ?? null,
      vueCompiler: Boolean(vue),
      moduleResolution: resolution.bundler,
      files: known.size,
      filesRead: store?.files.size ?? 0,
      bundlesSkipped: skippedBundles,
      residentMb: Math.round(process.memoryUsage().rss / 1048576),
    }),
    sync: (request) => store?.sync(request.changed ?? []) ?? { watch: [...restartInputs], warnings: [NO_TYPESCRIPT] },
    resolve: (request) => store?.resolve(request.query) ?? [],
    locate: (request) => store?.locate(request.file, request.line) ?? null,
    callers: (request) => store?.callers(request.symbol) ?? null,
  };

  // One request at a time, in arrival order, even while one waits for the warm-up.
  let queue = Promise.resolve();
  const input = createInterface({ input: process.stdin });
  // The server is leaving: what changed since is saved for the next one.
  input.on('close', () => {
    queue = queue.then(() => store?.save());
  });
  input.on('line', (line) => {
    let request;
    try {
      request = JSON.parse(line);
    } catch {
      return;
    }
    queue = queue.then(async () => {
      let response;
      try {
        const handler = handlers[request.op];
        response = { id: request.id, result: handler ? ((await handler(request)) ?? null) : null };
      } catch (error) {
        response = { id: request.id, error: String(error?.stack ?? error) };
      }
      process.stdout.write(JSON.stringify(response) + '\n');
    });
  });
}

if (mode === '--read') await serveReader();
else serveDriver();
