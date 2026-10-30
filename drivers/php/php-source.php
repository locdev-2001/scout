<?php

declare(strict_types=1);

const PHP_NAME_TOKENS = [T_STRING, T_NAME_QUALIFIED, T_NAME_FULLY_QUALIFIED, T_NAME_RELATIVE, T_STATIC];
const PHP_BUILTIN_TYPES = ['int', 'float', 'string', 'bool', 'array', 'mixed', 'void', 'null', 'callable', 'iterable', 'object', 'false', 'true', 'never', 'resource', 'integer', 'boolean', 'double'];

function isIdentifierText(string $text): bool
{
    return preg_match('/^[A-Za-z_\x80-\xff][A-Za-z0-9_\x80-\xff]*$/', $text) === 1;
}

function isNameToken(?PhpToken $token): bool
{
    return $token !== null && in_array($token->id, PHP_NAME_TOKENS, true);
}

/** A class-like declaration or a named function/method, with its line and token span. */
final class PhpScope
{
    public int $endLine = PHP_INT_MAX;

    public int $endToken = PHP_INT_MAX;

    /** Index of the `{` opening the body. */
    public int $bodyToken = PHP_INT_MAX;

    /** A trait's methods run under the using class's name. */
    public bool $isTrait = false;

    /** A closure outside any class or named function (a route file's): named by where it starts. */
    public bool $isFileClosure = false;

    /** What an anonymous class extends and implements, read from the source: it has no name to reflect on. */
    public ?array $anonymousParents = null;

    public function __construct(
        public readonly string $kind,
        public readonly ?string $className,
        public readonly ?string $functionName,
        public readonly int $startLine,
        public readonly int $startToken,
    ) {}

    public function symbol(): string
    {
        if ($this->kind === 'class') {
            return $this->className;
        }

        return $this->className !== null ? "{$this->className}::{$this->functionName}" : $this->functionName;
    }

    public function containsLine(int $line): bool
    {
        return $line >= $this->startLine && $line <= $this->endLine;
    }

    public function containsToken(int $index): bool
    {
        return $index >= $this->startToken && $index <= $this->endToken;
    }
}

/** What one PHP file declares and imports, read from the engine's own tokenizer. */
final class PhpFileModel
{
    /** @var list<PhpToken> tokens without whitespace and comments */
    public array $tokens = [];

    public string $namespace = '';

    /** @var array<string, string> lowercase alias => fully qualified class name */
    public array $imports = [];

    /** @var list<PhpScope> */
    public array $scopes = [];

    /** @var list<string> */
    public array $lines;

    /** @var array<string, string>|null */
    private ?array $unitHashes = null;

    public function __construct(public readonly string $path, public readonly string $source)
    {
        $this->lines = explode("\n", $source);
        foreach (PhpToken::tokenize($source) as $token) {
            if (! $token->isIgnorable()) {
                $this->tokens[] = $token;
            }
        }
        $this->readDeclarations();
    }

    private function readDeclarations(): void
    {
        $depth = 0;
        $openScopes = [];
        $pending = null;
        $count = count($this->tokens);

        for ($index = 0; $index < $count; $index++) {
            $token = $this->tokens[$index];
            $previous = $this->tokens[$index - 1] ?? null;

            if ($token->id === T_NAMESPACE && isNameToken($this->tokens[$index + 1] ?? null)) {
                $this->namespace = $this->tokens[$index + 1]->text;
            } elseif ($token->id === T_USE && $depth === 0) {
                $index = $this->readImports($index);
            } elseif (in_array($token->id, [T_CLASS, T_INTERFACE, T_TRAIT, T_ENUM], true)) {
                $name = $this->tokens[$index + 1] ?? null;
                if ($token->id === T_CLASS && $previous?->id === T_NEW) {
                    $outer = null;
                    foreach (array_reverse($openScopes) as [$scopeIndex]) {
                        $outer ??= $this->scopes[$scopeIndex]->kind === 'class' ? $this->scopes[$scopeIndex]->className : null;
                    }
                    $pending = new PhpScope('class', ($outer ?? $this->qualify('class'))."@anonymous_{$token->line}", null, $token->line, $index);
                    $pending->anonymousParents = $this->anonymousParents($index + 1, $outer);
                } elseif ($previous?->id !== T_DOUBLE_COLON && $previous?->id !== T_NEW && $name?->id === T_STRING) {
                    $pending = new PhpScope('class', $this->qualify($name->text), null, $token->line, $index);
                    $pending->isTrait = $token->id === T_TRAIT;
                }
            } elseif ($token->id === T_FN && $this->tokens[$index + 1]?->text === '(' && ($macro = $this->macroRegistered($index, $openScopes)) !== null) {
                $this->arrowFunction($index, $macro[0], $macro[1]);
            } elseif ($token->id === T_FN && $this->tokens[$index + 1]?->text === '(' && array_filter($openScopes, fn (array $open) => ! $this->scopes[$open[0]]->isFileClosure) === []) {
                $this->arrowFunction($index, null, "{$this->path}:{$token->line}");
            } elseif ($token->id === T_FUNCTION) {
                $nameIndex = ($this->tokens[$index + 1]->text ?? '') === '&' ? $index + 2 : $index + 1;
                $name = $this->tokens[$nameIndex] ?? null;
                $inFileCode = array_filter($openScopes, fn (array $open) => ! $this->scopes[$open[0]]->isFileClosure) === [];
                $macro = $name?->text === '(' ? $this->macroRegistered($index, $openScopes) : null;
                if ($macro !== null) {
                    $pending = new PhpScope('function', $macro[0], $macro[1], $token->line, $index);
                } elseif ($name?->text === '(' && $inFileCode) {
                    $pending = new PhpScope('function', null, "{$this->path}:{$token->line}", $token->line, $index);
                    $pending->isFileClosure = true;
                } elseif ($name !== null && $name->text !== '(') {
                    $enclosing = end($openScopes);
                    $isMethod = $enclosing !== false && $enclosing[1] === $depth && $this->scopes[$enclosing[0]]->kind === 'class';
                    $pending = $isMethod
                        ? new PhpScope('function', $this->scopes[$enclosing[0]]->className, $name->text, $token->line, $index)
                        : new PhpScope('function', null, $this->qualify($name->text), $token->line, $index);
                }
            } elseif ($token->text === '{' || $token->id === T_CURLY_OPEN || $token->id === T_DOLLAR_OPEN_CURLY_BRACES) {
                $depth++;
                if ($pending !== null && $token->text === '{') {
                    $pending->bodyToken = $index;
                    $this->scopes[] = $pending;
                    $openScopes[] = [count($this->scopes) - 1, $depth];
                    $pending = null;
                }
            } elseif ($token->text === '}') {
                $innermost = end($openScopes);
                if ($innermost !== false && $innermost[1] === $depth) {
                    array_pop($openScopes);
                    $this->scopes[$innermost[0]]->endLine = $token->line;
                    $this->scopes[$innermost[0]]->endToken = $index;
                }
                $depth--;
            } elseif ($token->text === ';' && $pending?->kind === 'function') {
                $pending = null;
            }
        }
    }

    /**
     * `Builder::macro('whereActive', function () {...})`: the closure becomes a method of that class, so the code it
     * calls has a caller. Returns [class, method name], or null for any other closure.
     */
    private function macroRegistered(int $index, array $openScopes): ?array
    {
        $before = $index - 1;
        if (($this->tokens[$before]->id ?? null) === T_STATIC) {
            $before--;
        }
        $name = $this->tokens[$before - 1] ?? null;
        $call = $this->tokens[$before - 3] ?? null;
        $operator = $this->tokens[$before - 4]->text ?? '';
        $receiver = $this->tokens[$before - 5] ?? null;
        if (($this->tokens[$before]->text ?? '') !== ',' || $name?->id !== T_CONSTANT_ENCAPSED_STRING || ($this->tokens[$before - 2]->text ?? '') !== '('
            || strcasecmp((string) $call?->text, 'macro') !== 0 || $operator !== '::' || ! (isNameToken($receiver) || isIdentifierText((string) $receiver?->text))) {
            return null;
        }
        $currentClass = null;
        foreach (array_reverse($openScopes) as [$scopeIndex]) {
            $currentClass ??= $this->scopes[$scopeIndex]->kind === 'class' ? $this->scopes[$scopeIndex]->className : null;
        }
        $class = $this->resolveClassName($receiver->text, $currentClass);

        return $class === null ? null : [$class, substr($name->text, 1, -1)];
    }

    /** `fn (...) => expression`: a scope from `fn` to the end of its expression. */
    private function arrowFunction(int $index, ?string $className, string $functionName): void
    {
        $close = $this->matchingClose($index + 1);
        $arrow = $close === null ? null : $close + 1;
        while ($arrow !== null && isset($this->tokens[$arrow]) && $this->tokens[$arrow]->id !== T_DOUBLE_ARROW && $this->tokens[$arrow]->text !== '{') {
            $arrow++; // past a return type
        }
        if ($arrow === null || ($this->tokens[$arrow]->id ?? null) !== T_DOUBLE_ARROW) {
            return;
        }
        $end = $this->expressionEnd($arrow + 1) - 1;
        $scope = new PhpScope('function', $className, $functionName, $this->tokens[$index]->line, $index);
        $scope->isFileClosure = $className === null;
        $scope->bodyToken = $arrow;
        $scope->endToken = $end;
        $scope->endLine = $this->tokens[$end]->line;
        $this->scopes[] = $scope;
    }

    /** Index of the `)` closing the `(` at $open. */
    private function matchingClose(int $open): ?int
    {
        $depth = 0;
        for ($index = $open, $count = count($this->tokens); $index < $count; $index++) {
            $text = $this->tokens[$index]->text;
            if ($text === '(') {
                $depth++;
            } elseif ($text === ')' && --$depth === 0) {
                return $index;
            }
        }

        return null;
    }

    /** `new class(...) extends A implements B, C {`: A, B and C, resolved. */
    private function anonymousParents(int $index, ?string $outer): array
    {
        $parents = [];
        $depth = 0;
        for (; isset($this->tokens[$index]) && ($depth > 0 || $this->tokens[$index]->text !== '{'); $index++) {
            $token = $this->tokens[$index];
            $depth += $token->text === '(' ? 1 : ($token->text === ')' ? -1 : 0);
            $before = $this->tokens[$index - 1];
            if ($depth === 0 && isNameToken($token) && ($before->id === T_EXTENDS || $before->id === T_IMPLEMENTS || $before->text === ',')) {
                $parents[] = $this->resolveClassName($token->text, $outer);
            }
        }

        return array_values(array_filter($parents));
    }

    private function readImports(int $useIndex): int
    {
        $texts = [];
        for ($index = $useIndex + 1; isset($this->tokens[$index]) && $this->tokens[$index]->text !== ';'; $index++) {
            $texts[] = $this->tokens[$index]->text;
        }
        $clause = preg_replace('/\s*([\\\\{},])\s*/', '$1', implode(' ', $texts));
        if (preg_match('/^(function|const)\s/i', $clause)) {
            return $index;
        }

        $prefix = '';
        $items = $clause;
        if (preg_match('/^(.*?)\\\\?\{(.*)\}$/s', $clause, $group)) {
            $prefix = trim($group[1], '\\').'\\';
            $items = $group[2];
        }
        foreach (explode(',', $items) as $item) {
            $parts = preg_split('/\s+as\s+/i', trim($item));
            $name = ltrim($prefix.$parts[0], '\\');
            if ($name === '' || $name === '\\') {
                continue;
            }
            $alias = $parts[1] ?? substr(strrchr('\\'.$name, '\\'), 1);
            $this->imports[strtolower($alias)] = $name;
        }

        return $index;
    }

    private function qualify(string $name): string
    {
        return $this->namespace === '' ? $name : "{$this->namespace}\\{$name}";
    }

    /** Resolves a class name as written in this file; `self`/`static`/`parent` are relative to $currentClass. */
    public function resolveClassName(string $name, ?string $currentClass): ?string
    {
        $lower = strtolower($name);
        if ($name === '' || in_array($lower, PHP_BUILTIN_TYPES, true)) {
            return null;
        }
        if ($lower === 'self' || $lower === 'static' || $lower === '$this') {
            return $currentClass;
        }
        if ($lower === 'parent') {
            return $currentClass !== null ? (rescueReflection(fn () => get_parent_class($currentClass)) ?: null) : null;
        }
        if ($name[0] === '\\') {
            return substr($name, 1);
        }
        if (str_starts_with($lower, 'namespace\\')) {
            return $this->qualify(substr($name, 10));
        }
        $segments = explode('\\', $name, 2);
        $imported = $this->imports[strtolower($segments[0])] ?? null;
        if ($imported !== null) {
            return isset($segments[1]) ? "{$imported}\\{$segments[1]}" : $imported;
        }

        return $this->qualify($name);
    }

    /**
     * Fingerprints of the units other files' facts can rely on: the body of every function or method
     * (`body:<symbol>`) and the shape of every class, which is the class without its method bodies
     * (`shape:<class>`): inheritance, properties, constants, signatures and their docblocks.
     * Line positions are not part of it, so moving code up or down changes nothing.
     *
     * @return array<string, string>
     */
    public function unitHashes(): array
    {
        if ($this->unitHashes !== null) {
            return $this->unitHashes;
        }
        $hashes = [];
        foreach ($this->scopes as $scope) {
            if ($scope->kind === 'function') {
                $hashes['body:'.strtolower($scope->symbol())] = hash('xxh128', $this->spanText($scope->bodyToken, $scope->endToken));
            }
        }
        foreach ($this->scopes as $scope) {
            if ($scope->kind !== 'class') {
                continue;
            }
            $shape = '';
            $cursor = $scope->startToken;
            foreach ($this->scopes as $member) {
                if ($member->kind === 'function' && $member->className === $scope->className && $member->bodyToken > $cursor && $member->endToken <= $scope->endToken) {
                    $shape .= $this->spanText($cursor, $member->bodyToken - 1);
                    $cursor = $member->endToken + 1;
                }
            }
            $shape .= $this->spanText($cursor, $scope->endToken);
            $hashes['shape:'.strtolower($scope->className)] = hash('xxh128', $shape);
        }
        // How names written in the file resolve: its namespace and imports.
        $hashes['head'] = hash('xxh128', serialize([$this->namespace, $this->imports]));
        // Everything except function bodies: top-level code, class shapes, signatures.
        $outside = '';
        $cursor = 0;
        foreach ($this->scopes as $scope) {
            if ($scope->kind === 'function' && $scope->bodyToken >= $cursor && $scope->endToken < PHP_INT_MAX) {
                $outside .= $this->spanText($cursor, $scope->bodyToken - 1);
                $cursor = $scope->endToken + 1;
            }
        }
        $hashes['outside'] = hash('xxh128', $outside.$this->spanText($cursor, count($this->tokens) - 1));

        return $this->unitHashes = $hashes;
    }

    /** Source text from the start of token $from to the end of token $to, docblocks and comments included. */
    private function spanText(int $from, int $to): string
    {
        $first = $this->tokens[$from] ?? null;
        $last = $this->tokens[min($to, count($this->tokens) - 1)] ?? null;
        if ($first === null || $last === null || $last->pos < $first->pos) {
            return '';
        }

        return substr($this->source, $first->pos, $last->pos + strlen($last->text) - $first->pos);
    }

    /** @return array<string, true> lowercase names of the traits this file declares */
    public function traitNames(): array
    {
        $traits = [];
        foreach ($this->scopes as $scope) {
            if ($scope->isTrait) {
                $traits[strtolower($scope->className)] = true;
            }
        }

        return $traits;
    }

    public function innermostScopeAtLine(int $line, ?string $kind = null): ?PhpScope
    {
        return $this->innermost(fn (PhpScope $scope) => $scope->containsLine($line) && ($kind === null || $scope->kind === $kind));
    }

    public function innermostScopeAtToken(int $index, ?string $kind = null): ?PhpScope
    {
        return $this->innermost(fn (PhpScope $scope) => $scope->containsToken($index) && ($kind === null || $scope->kind === $kind));
    }

    private function innermost(callable $matches): ?PhpScope
    {
        $found = null;
        foreach ($this->scopes as $scope) {
            if ($matches($scope) && ($found === null || $scope->startToken >= $found->startToken)) {
                $found = $scope;
            }
        }

        return $found;
    }

    public function classAtToken(int $index): ?string
    {
        return $this->innermostScopeAtToken($index, 'class')?->className;
    }

    /** @return array{symbol: string, file: string, line: int, kind?: string} */
    public function symbolAtLine(int $line): array
    {
        $scope = $this->innermostScopeAtLine($line);
        if ($scope === null) {
            return ['symbol' => $this->path, 'file' => $this->path, 'line' => 1, 'kind' => 'file'];
        }

        return ['symbol' => $scope->symbol(), 'file' => $this->path, 'line' => $scope->startLine];
    }

    /** Index of the `(` matching the `)` at $closeIndex. */
    public function matchingOpen(int $closeIndex): ?int
    {
        $depth = 0;
        for ($index = $closeIndex; $index >= 0; $index--) {
            $text = $this->tokens[$index]->text;
            if ($text === ')' || $text === ']') {
                $depth++;
            } elseif ($text === '(' || $text === '[') {
                if (--$depth === 0) {
                    return $index;
                }
            }
        }

        return null;
    }

    /** Index of the token ending the expression that starts at $startIndex (`;`, or a closing bracket of the enclosing call). */
    public function expressionEnd(int $startIndex): int
    {
        $depth = 0;
        $count = count($this->tokens);
        for ($index = $startIndex; $index < $count; $index++) {
            $text = $this->tokens[$index]->text;
            if (in_array($text, ['(', '[', '{'], true) || $this->tokens[$index]->id === T_CURLY_OPEN) {
                $depth++;
            } elseif (in_array($text, [')', ']', '}'], true)) {
                if (--$depth < 0) {
                    return $index;
                }
            } elseif ($depth === 0 && ($text === ';' || $text === ',')) {
                return $index;
            }
        }

        return $count;
    }
}

/** Reflection autoloads classes; a broken class file must not take the driver down. */
function rescueReflection(callable $probe): mixed
{
    try {
        return $probe();
    } catch (Throwable) {
        return null;
    }
}

final class PhpFileCache
{
    /** @var array<string, PhpFileModel|null> read files, the most recently asked for last */
    private array $models = [];

    /** @var list<string>|null paths Composer autoloads only for development: tests and their helpers */
    private ?array $testPaths = null;

    /** @var array<string, list<int>>|null without ripgrep: lowercase word => ids of the files it appears in */
    private ?array $wordIndex = null;

    /** @var list<string> the files the word index was built from, by id */
    private array $indexedFiles = [];

    /** @var array<string, true> files changed since the word index was built: always searched by reading them */
    private array $changedSinceIndex = [];

    /**
     * @param  int|null  $modelsKept  read files kept for later questions, the least recently asked for dropped first;
     *                                null keeps every one (a model holds about 24 times its file's size)
     */
    public function __construct(public readonly string $repoRoot, private readonly ?int $modelsKept = null) {}

    /**
     * Files the core reported changed (added, removed or edited), so the word index never answers from old text.
     *
     * @param  list<string>  $paths
     */
    public function changed(array $paths): void
    {
        if ($this->wordIndex === null) {
            return;
        }
        foreach ($paths as $path) {
            if (basename($path) === '.gitignore') {
                // What counts as a repo file may have changed anywhere: build the index again when next needed.
                [$this->wordIndex, $this->indexedFiles, $this->changedSinceIndex] = [null, [], []];

                return;
            }
            $this->changedSinceIndex[$path] = true;
        }
    }

    /** Whether a file belongs to the tests (a helper trait in tests/ included, not only test classes). */
    public function isTestPath(string $relativePath): bool
    {
        $this->testPaths ??= self::readTestPaths($this->repoRoot);
        foreach ($this->testPaths as $testPath) {
            if (str_starts_with($relativePath, $testPath)) {
                return true;
            }
        }

        return false;
    }

    /** @return list<string> `autoload-dev` of composer.json; `tests/` when it has none */
    private static function readTestPaths(string $repoRoot): array
    {
        $composer = json_decode((string) @file_get_contents("{$repoRoot}/composer.json"), true);
        $development = is_array($composer) ? ($composer['autoload-dev'] ?? null) : null;
        if (! is_array($development)) {
            return ['tests/'];
        }
        $paths = [];
        foreach (['psr-4', 'psr-0'] as $standard) {
            foreach ($development[$standard] ?? [] as $directories) {
                array_push($paths, ...array_map(fn ($directory) => rtrim((string) $directory, '/').'/', (array) $directories));
            }
        }
        array_push($paths, ...array_map('strval', $development['classmap'] ?? []), ...array_map('strval', $development['files'] ?? []));

        return array_values(array_unique(array_map(fn (string $path) => (string) preg_replace('#^\./#', '', $path), $paths)));
    }

    public function get(string $relativePath): ?PhpFileModel
    {
        if (array_key_exists($relativePath, $this->models)) {
            $model = $this->models[$relativePath];
            if ($this->modelsKept !== null) {
                unset($this->models[$relativePath]);
                $this->models[$relativePath] = $model;
            }

            return $model;
        }
        if ($this->modelsKept !== null && count($this->models) >= $this->modelsKept) {
            unset($this->models[array_key_first($this->models)]);
        }
        $source = @file_get_contents($this->repoRoot.'/'.$relativePath);

        return $this->models[$relativePath] = $source === false ? null : new PhpFileModel($relativePath, $source);
    }

    /** Drops what was read of a file that changed. */
    public function forget(string $relativePath): void
    {
        unset($this->models[$relativePath]);
    }

    public function relativePath(string $absolutePath): ?string
    {
        // Code loaded from the checkout whose vendor/ this one shares (helpers Composer requires up front).
        $sharedRoot = sharedCheckoutRoot($this->repoRoot);
        if ($sharedRoot !== null) {
            $absolutePath = checkoutPath($absolutePath, $sharedRoot, $this->repoRoot);
        }
        $prefix = $this->repoRoot.'/';

        return str_starts_with($absolutePath, $prefix) ? substr($absolutePath, strlen($prefix)) : null;
    }

    public function forClass(string $className): ?PhpFileModel
    {
        $file = rescueReflection(fn () => (new ReflectionClass($className))->getFileName());
        $relative = is_string($file) ? $this->relativePath($file) : null;

        return $relative !== null ? $this->get($relative) : null;
    }

    /** @return list<string> every PHP file of the repo that version control does not ignore */
    public function phpFiles(): array
    {
        $output = self::hasRipgrep()
            ? $this->runHere(['rg', '--files', '--hidden', '--type', 'php'])
            : str_replace("\0", "\n", $this->runHere(['git', 'ls-files', '-co', '--exclude-standard', '-z', '--', '*.php', '*.phtml']));

        return array_values(array_filter(explode("\n", $output), fn (string $line) => $line !== ''));
    }

    /** Whether ripgrep is on PATH; without it the files are listed by git and searched here (slower, same answer). */
    private static function hasRipgrep(): bool
    {
        static $found = null;
        if ($found === null) {
            $found = false;
            foreach (explode(PATH_SEPARATOR, (string) getenv('PATH')) as $folder) {
                $found = $found || ($folder !== '' && (is_executable("{$folder}/rg") || is_executable("{$folder}/rg.exe")));
            }
        }

        return $found;
    }

    /** @param  list<string>  $command */
    private function runHere(array $command): string
    {
        $process = proc_open($command, [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes, $this->repoRoot);
        if (! is_resource($process)) {
            throw new RuntimeException("cannot run {$command[0]}");
        }
        $output = (string) stream_get_contents($pipes[1]);
        stream_get_contents($pipes[2]);
        proc_close($process);

        return $output;
    }

    /**
     * What `rg --files-with-matches` would list for these arguments (`-e`, `--ignore-case`, `--word-regexp`,
     * `--fixed-strings`), found by reading the files.
     *
     * @param  list<string>  $arguments
     * @param  list<string>  $paths
     * @return list<string>
     */
    private function searchHere(array $arguments, array $paths): array
    {
        $patterns = [];
        foreach ($arguments as $position => $argument) {
            if ($argument === '-e') {
                $patterns[] = $arguments[$position + 1];
            }
        }
        $fixed = in_array('--fixed-strings', $arguments, true);
        $alternatives = array_map(fn (string $pattern) => $fixed ? preg_quote($pattern, '#') : str_replace('#', '\\#', $pattern), $patterns);
        $expression = '(?:'.implode('|', $alternatives).')';
        if (in_array('--word-regexp', $arguments, true)) {
            $expression = "\\b{$expression}\\b";
        }
        $regex = '#'.$expression.'#'.(in_array('--ignore-case', $arguments, true) ? 'i' : '');

        return array_values(array_filter($paths, fn (string $path) => preg_match($regex, (string) @file_get_contents("{$this->repoRoot}/{$path}")) === 1));
    }

    /**
     * Without ripgrep, a whole-word search would read every PHP file for each name a question looks up. The files
     * that can match a word are those where it appears between non-word characters, found in an index of every
     * word, built once; searchHere() then checks only those, so the answer is the same. Null when the search is
     * not for whole words made of `\w` characters.
     *
     * @param  list<string>  $arguments
     * @return list<string>|null
     */
    private function candidates(array $arguments): ?array
    {
        if (! in_array('--word-regexp', $arguments, true) || ! in_array('--fixed-strings', $arguments, true)) {
            return null;
        }
        $words = [];
        foreach ($arguments as $position => $argument) {
            if ($argument === '-e') {
                $word = (string) ($arguments[$position + 1] ?? '');
                if (preg_match('/^\w+$/', $word) !== 1) {
                    return null;
                }
                $words[] = strtolower($word);
            }
        }
        if ($this->wordIndex === null) {
            $this->buildWordIndex();
        }
        $found = [];
        foreach ($words as $word) {
            foreach ($this->wordIndex[$word] ?? [] as $id) {
                $found[$this->indexedFiles[$id]] = true;
            }
        }
        foreach (array_keys($this->changedSinceIndex) as $path) {
            // The extensions phpFiles() lists (git pathspecs are case-sensitive).
            $found[$path] = is_file("{$this->repoRoot}/{$path}") && preg_match('/\.(php|phtml)$/', $path) === 1;
        }

        return array_keys(array_filter($found));
    }

    /** Frees the word index (tens of MB on a large repo) once no search needs it; a later search builds it again. */
    public function forgetWordIndex(): void
    {
        [$this->wordIndex, $this->indexedFiles, $this->changedSinceIndex] = [null, [], []];
    }

    private function buildWordIndex(): void
    {
        $started = microtime(true);
        [$this->wordIndex, $this->indexedFiles, $this->changedSinceIndex] = [[], $this->phpFiles(), []];
        foreach ($this->indexedFiles as $id => $path) {
            preg_match_all('/\w+/', (string) @file_get_contents("{$this->repoRoot}/{$path}"), $matches);
            foreach (array_unique(array_map('strtolower', $matches[0])) as $word) {
                $this->wordIndex[$word][] = $id;
            }
        }
        profilePhase('word index: built', $started, ['files' => count($this->indexedFiles), 'words' => count($this->wordIndex)]);
    }

    /**
     * Files under the repo matching every pattern set, using ripgrep (honours .gitignore, so vendor/ is skipped).
     *
     * @param  list<string>  $arguments
     * @param  list<string>|null  $paths  restrict the search to these files
     * @return list<string>
     */
    public function ripgrep(array $arguments, ?array $paths = null): array
    {
        if ($paths === []) {
            return [];
        }
        $output = self::hasRipgrep()
            ? $this->runHere(['rg', '--files-with-matches', '--no-messages', '--hidden', '--type', 'php', ...$arguments, '--', ...($paths ?? ['.'])])
            : implode("\n", $this->searchHere($arguments, $paths ?? $this->candidates($arguments) ?? $this->phpFiles()));

        $files = [];
        foreach (explode("\n", trim($output)) as $line) {
            if ($line !== '') {
                $files[] = str_starts_with($line, './') ? substr($line, 2) : $line;
            }
        }
        sort($files);

        return $files;
    }
}
