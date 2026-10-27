<?php

declare(strict_types=1);

/**
 * Runs in a forked child. Turns source files and classes into plain arrays the long-lived parent can keep,
 * each with the files it depends on, so the parent knows exactly what to recompute when a file changes.
 */
final class FactExtraction
{
    private const NOT_FUNCTION_CALLS = ['function', 'fn', 'if', 'elseif', 'while', 'for', 'foreach', 'switch', 'match', 'catch', 'return', 'echo', 'print', 'array', 'list', 'isset', 'unset', 'empty', 'exit', 'die', 'include', 'require', 'include_once', 'require_once', 'new', 'use', 'declare'];

    private readonly TypeInference $inference;

    /** @var array<string, array<string, string>> unit marker => concrete unit ids with their fingerprints */
    private array $unitCache = [];

    public function __construct(private readonly PhpFileCache $files)
    {
        $this->inference = new TypeInference($files);
    }

    /**
     * scopes:    [symbol, startLine, endLine, isClass, isTest]
     * sites:     [lowercase name, line, kind, types|null, confidence|null, scope index or -1, token index, question positions]
     * classes:   lowercase name => [declared name, lowercase ancestors]
     * units:     fingerprints of this file's own units (see PhpFileModel::unitHashes)
     * questions: list of [question, answer hash, unit id => true for every unit it relied on]
     *
     * @return array<string, mixed>|null
     */
    /**
     * $previous is this file's facts before its latest change. Call sites in a function whose body, class
     * shape and file head (namespace, imports) are unchanged, and whose questions relied on no changed unit,
     * are carried over with their positions shifted instead of being inferred again; so are the ancestry
     * answers of classes whose shape is unchanged. $changedPaths are all files changed in the same update.
     *
     * @param  list<string>  $changedPaths
     */
    public function fileFacts(string $path, ?array $previous = null, array $changedPaths = []): ?array
    {
        $file = $this->files->get($path);
        if ($file === null) {
            return null;
        }
        $this->forget();
        $units = $file->unitHashes();
        $carry = $previous === null ? null : new CarriedFacts($previous, $units, $path, $changedPaths);

        $scopes = [];
        $declared = [];
        $carriedRecords = [];
        foreach ($file->scopes as $index => $scope) {
            $class = $scope->className;
            $kept = $class === null ? null : $carry?->ancestry($scope, $class);
            if ($kept !== null) {
                [$ancestors, $isTest, $record] = $kept;
                $carriedRecords[$record[0]] = $record;
            } else {
                [$ancestors, $isTest] = match (true) {
                    $class === null => [[], false],
                    $scope->anonymousParents !== null => [array_map('strtolower', $scope->anonymousParents), false],
                    default => $this->ancestry($class),
                };
            }
            $scopes[] = [$scope->symbol(), $scope->startLine, $scope->endLine, $scope->kind === 'class', $isTest, $scope->bodyToken];
            if ($scope->kind === 'class') {
                // An anonymous class keeps its parents as written too: autoloading needs a class name's real case.
                $declared[strtolower($class)] = $scope->anonymousParents === null ? [$class, $ancestors] : [$class, $ancestors, $scope->anonymousParents];
            }
        }
        $carried = $carry?->sitesFor($file) ?? [];

        $sites = [];
        $mentions = [];
        $siteQuestions = [];
        foreach ($file->tokens as $index => $token) {
            if (isNameToken($token)) {
                $mentions[strtolower(substr(strrchr('\\'.$token->text, '\\'), 1))] = true;
            }
            if (array_key_exists($index, $carried)) {
                if ($carried[$index] !== null) {
                    [$site, $records] = $carried[$index];
                    $sites[] = $site;
                    $siteQuestions[] = array_column($records, 0);
                    foreach ($records as $record) {
                        $carriedRecords[$record[0]] ??= $record;
                    }
                }

                continue;
            }
            $this->inference->answered = [];
            $site = $this->siteFacts($file, $index);
            if ($site !== null) {
                $sites[] = $site;
                $siteQuestions[] = array_keys($this->inference->answered);
            }
        }

        $questions = [];
        $positions = [];
        foreach ($this->inference->questions as $question => [$answer, $questionUnits]) {
            $positions[$question] = count($questions);
            $questions[] = [$question, self::answerHash($answer), $this->concreteUnits($questionUnits)];
        }
        foreach ($carriedRecords as $question => $record) {
            if (! isset($positions[$question])) {
                $positions[$question] = count($questions);
                $questions[] = $record;
            }
        }
        foreach ($sites as $index => $site) {
            $sites[$index][] = array_map(fn (string $question) => $positions[$question], $siteQuestions[$index]);
        }
        $reliedOn = [];
        foreach ($questions as [, , $questionUnits]) {
            $reliedOn += $questionUnits;
        }

        return [
            'scopes' => $scopes,
            'sites' => $sites,
            'mentions' => array_keys($mentions),
            'classes' => $declared,
            'hash' => self::contentHash($this->files->repoRoot.'/'.$path),
            'units' => $units,
            'questions' => $questions,
            'deps' => self::unitPaths($reliedOn, [$path]),
            'carried' => count(array_filter($carried)),
        ];
    }

    /** [lowercase name, line, kind, types|null, confidence|null, scope index or -1, token index] for a call site. */
    private function siteFacts(PhpFileModel $file, int $index): ?array
    {
        $site = $this->site($file, $index);
        if ($site === null) {
            return null;
        }
        [$name, $kind, $receiver] = $site;
        $line = $file->tokens[$index]->line;

        return [$name, $line, $kind, $receiver['types'] ?? null, $receiver['confidence'] ?? null, $this->scopeIndexAt($file, $line), $index];
    }

    /**
     * Recomputes only the call sites at these token indexes of an unchanged file, after an answer they relied
     * on changed. Returns each site with the questions it asked: [site, list of [question, answer hash, units]].
     *
     * @param  list<int>  $tokenIndexes
     * @return array<int, array{0: array|null, 1: list<array>}>
     */
    public function refreshSites(string $path, array $tokenIndexes): array
    {
        $started = microtime(true);
        $file = $this->files->get($path);
        profilePhase('refresh: read file', $started, ['path' => $path]);
        $started = microtime(true);
        $refreshed = [];
        foreach ($tokenIndexes as $index) {
            $this->forget();
            $this->inference->answered = [];
            $siteStarted = microtime(true);
            $site = $file === null ? null : $this->siteFacts($file, $index);
            if (microtime(true) - $siteStarted > 0.01) {
                profilePhase('refresh: slow site', $siteStarted, ['path' => $path, 'line' => $site[1] ?? null, 'name' => $site[0] ?? null, 'questions' => array_keys($this->inference->answered), 'loadedFiles' => count(get_included_files())]);
            }
            $questions = [];
            foreach (array_keys($this->inference->answered) as $question) {
                [$answer, $units] = $this->inference->questions[$question];
                $questions[] = [$question, self::answerHash($answer), $this->concreteUnits($units)];
            }
            $refreshed[$index] = [$site, $questions];
        }
        profilePhase('refresh: sites', $started, ['path' => $path, 'sites' => count($tokenIndexes)]);

        return $refreshed;
    }

    public static function contentHash(string $absolutePath): string
    {
        return (string) @hash_file('xxh128', $absolutePath);
    }

    public static function answerHash(mixed $answer): string
    {
        return hash('xxh128', serialize($answer));
    }

    /** @param array<string, string> $units */
    private static function unitPaths(array $units, array $paths = []): array
    {
        foreach (array_keys($units) as $unit) {
            // A class that does not exist yet is tracked under its own id, so declaring it later finds these facts.
            $paths[] = str_contains($unit, '#') ? strstr($unit, '#', true) : $unit;
        }

        return array_values(array_unique($paths));
    }

    private function forget(): void
    {
        $this->inference->questions = [];
        $this->inference->takeUnits();
    }

    /**
     * Lowercase ancestors of $class (parents, interfaces and the traits it uses, which lend it their methods), and
     * whether it is a PHPUnit test.
     *
     * @return array{0: list<string>, 1: bool}
     */
    private function ancestry(string $class): array
    {
        $this->inference->beginQuestion();
        $this->inference->noteUnit("class:{$class}");
        $ancestors = array_map('strtolower', array_values(rescueReflection(fn () => [...@class_parents($class), ...@class_implements($class), ...self::traitsOf($class)]) ?? []));

        return $this->inference->answer("ancestry|{$class}", [$ancestors, in_array('phpunit\\framework\\testcase', $ancestors, true)]);
    }

    /**
     * Asks recorded questions again on the code as it is now.
     *
     * @param  list<string>  $questions
     * @return array<string, array{0: string, 1: array<string, true>}> question => [answer hash, units it relied on]
     */
    public function answers(array $questions): array
    {
        $answers = [];
        foreach ($questions as $question) {
            $this->forget();
            if (str_starts_with($question, 'ancestry|')) {
                $this->ancestry(substr($question, 9));
            } else {
                $this->inference->ask($question);
            }
            [$answer, $units] = $this->inference->questions[$question];
            $answers[$question] = [self::answerHash($answer), $this->concreteUnits($units)];
        }

        return $answers;
    }

    /**
     * Unit markers noted during inference => concrete unit ids: `<path>#shape:<class>`, `<path>#body:<symbol>`,
     * `<path>#file`, or `missing-class:<name>` for a class that does not exist yet. No fingerprint is needed here:
     * when a file changes, the parent compares that file's own unit fingerprints before and after.
     *
     * @param  array<string, true>  $markers
     * @return array<string, true>
     */
    private function concreteUnits(array $markers): array
    {
        $units = [];
        foreach (array_keys($markers) as $marker) {
            $units += $this->unitCache[$marker] ??= $this->expandUnit($marker);
        }

        return $units;
    }

    private function expandUnit(string $marker): array
    {
        [$kind, $value] = explode(':', $marker, 2);
        if ($kind === 'body') {
            [$path, $symbol] = explode('|', $value, 2);

            return ["{$path}#body:{$symbol}" => true];
        }
        if ($kind === 'file') {
            $relative = $this->projectPaths([$value])[0] ?? null;

            return $relative === null ? [] : ["{$relative}#file" => true];
        }
        if (! rescueReflection(fn () => class_exists($value) || interface_exists($value) || trait_exists($value))) {
            return ['missing-class:'.strtolower(ltrim($value, '\\')) => true];
        }
        $units = [];
        foreach (self::hierarchyNames($value) as $name) {
            $file = rescueReflection(fn () => (new ReflectionClass($name))->getFileName());
            $relative = is_string($file) ? ($this->projectPaths([$file])[0] ?? null) : null;
            if ($relative !== null) {
                $units["{$relative}#shape:".strtolower($name)] = true;
            }
        }

        return $units;
    }

    private function scopeIndexAt(PhpFileModel $file, int $line): int
    {
        $found = -1;
        foreach ($file->scopes as $index => $scope) {
            if ($scope->containsLine($line) && ($found === -1 || $scope->startToken >= $file->scopes[$found]->startToken)) {
                $found = $index;
            }
        }

        return $found;
    }

    /** @return array{0: string, 1: string, 2: array|null}|null [lowercase name, kind, receiver types] */
    private function site(PhpFileModel $file, int $index): ?array
    {
        $token = $file->tokens[$index];
        $previous = $file->tokens[$index - 1] ?? null;
        $next = $file->tokens[$index + 1] ?? null;

        if ($token->id === T_NEW && isNameToken($next)) {
            return ['__construct', 'new', $this->inference->staticReceiverTypes($file, $index + 1)];
        }
        // `$this->$method()`, `$class::$method()`: the method is named at run time, so it may be any of the class's.
        if ($token->id === T_VARIABLE && $next?->text === '(' && in_array($previous?->text, ['->', '?->', '::'], true)) {
            $receiver = $previous->text === '::' ? $this->inference->staticReceiverTypes($file, $index - 2) : $this->inference->expressionTypes($file, $index - 2);

            return [FactStore::DYNAMIC_NAME, 'dynamic', $receiver];
        }
        if ($next?->text === '(' && ($token->text === ')' ? $this->callsResult($file, $index) : $token->id === T_VARIABLE && $token->text !== '$this')) {
            return ['__invoke', 'call', $this->inference->expressionTypes($file, $index)];
        }
        if ($next?->text === '(' && (isNameToken($token) || isIdentifierText($token->text))) {
            $name = strtolower($token->text);
            if ($previous?->text === '->' || $previous?->text === '?->') {
                return [$name, 'call', $this->inference->expressionTypes($file, $index - 2)];
            }
            if ($previous?->text === '::') {
                // `parent::m()`, `self::m()`, `Foo::m()` run that class's method; only `static::m()` and `$x::m()`
                // are resolved at run time and so may reach an override.
                $receiver = $file->tokens[$index - 2] ?? null;
                $late = $receiver === null || $receiver->id === T_VARIABLE || $receiver->text === ')' || strtolower($receiver->text) === 'static';

                return [$name, $late ? 'late static' : 'static', $this->inference->staticReceiverTypes($file, $index - 2)];
            }
            if ($previous?->id !== T_FUNCTION && $previous?->id !== T_NEW && $previous?->id !== T_FN && ! in_array($name, self::NOT_FUNCTION_CALLS, true)) {
                return $this->functionSite($file, $token->text);
            }
        }
        if ($token->id === T_CONSTANT_ENCAPSED_STRING) {
            return $this->callableSite($file, $index);
        }

        return null;
    }

    /** `make()(...)`, `$list[0](...)`: the parenthesis at $closeIndex ends a value that is then called. */
    private function callsResult(PhpFileModel $file, int $closeIndex): bool
    {
        $open = $file->matchingOpen($closeIndex);
        $before = $open === null ? null : ($file->tokens[$open - 1] ?? null);
        if ($before === null || in_array(strtolower($before->text), self::NOT_FUNCTION_CALLS, true)) {
            return false;
        }

        return isNameToken($before) || isIdentifierText($before->text) || $before->id === T_VARIABLE || $before->text === ')' || $before->text === ']';
    }

    /** A function call: the names PHP would try, namespaced first, then global. */
    private function functionSite(PhpFileModel $file, string $written): array
    {
        $candidates = str_contains($written, '\\')
            ? [ltrim((string) $file->resolveClassName($written, null), '\\')]
            : array_unique([$file->namespace === '' ? $written : "{$file->namespace}\\{$written}", $written]);
        $short = substr(strrchr('\\'.$written, '\\'), 1);

        return [strtolower($short), 'function', ['types' => array_map('strtolower', $candidates), 'confidence' => 'exact']];
    }

    /** `[Foo::class, 'method']`, `[$object, 'method']` and `'Foo@method'`. */
    private function callableSite(PhpFileModel $file, int $index): ?array
    {
        $token = $file->tokens[$index];
        $literal = substr($token->text, 1, -1);
        if (preg_match('/^\\\\?([\w\\\\]+)@(\w+)$/', $literal, $match)) {
            return [strtolower($match[2]), 'callable', ['types' => [$match[1]], 'confidence' => 'likely']];
        }
        if (! isIdentifierText($literal) || ($file->tokens[$index - 1]->text ?? '') !== ',') {
            return null;
        }
        $owner = $file->tokens[$index - 2] ?? null;
        // Only once the string has an owner: storedInProperty() walks back to the statement's start, and a
        // translation file is one statement whose every key is a name after a comma.
        $kind = fn () => $this->storedInProperty($file, $index) ? 'stored callable' : 'callable';
        if ($owner?->id === T_CLASS && ($file->tokens[$index - 3]->text ?? '') === '::') {
            return [strtolower($literal), $kind(), $this->inference->staticReceiverTypes($file, $index - 4)];
        }
        $property = $owner !== null && isIdentifierText($owner->text) && in_array($file->tokens[$index - 3]->text ?? '', ['->', '?->'], true);
        if ($owner?->id === T_VARIABLE || $property) {
            return [strtolower($literal), $kind(), $this->inference->expressionTypes($file, $index - 2)];
        }

        return null;
    }

    /** Whether the statement holding token $index assigns to a property: `$this->handler = Closure::fromCallable(...)`. */
    private function storedInProperty(PhpFileModel $file, int $index): bool
    {
        $depth = 0;
        for ($start = $index - 1; $start >= 0; $start--) {
            $text = $file->tokens[$start]->text;
            if ($text === ')' || $text === ']') {
                $depth++;
            } elseif ($text === '(' || $text === '[') {
                $depth = max(0, $depth - 1);
            } elseif ($depth === 0 && in_array($text, [';', '{', '}'], true)) {
                break;
            }
        }
        $first = $file->tokens[$start + 1] ?? null;
        $arrow = $file->tokens[$start + 2]->text ?? '';
        $assigned = $file->tokens[$start + 4]->text ?? '';

        return $first?->id === T_VARIABLE && in_array($arrow, ['->', '::'], true) && ($assigned === '=' || $assigned === '[');
    }

    /**
     * Repo-relative paths the core watches; Composer's vendor/ is left out because the driver restarts
     * whenever vendor/composer/installed.json changes.
     *
     * @param  list<string>  $absolutePaths
     * @return list<string>
     */
    private function projectPaths(array $absolutePaths): array
    {
        $paths = [];
        foreach ($absolutePaths as $file) {
            $relative = $this->files->relativePath($file);
            if ($relative !== null && ! str_starts_with($relative, 'vendor/')) {
                $paths[$relative] = true;
            }
        }

        return array_keys($paths);
    }

    /** @return list<string> the class, its ancestors, interfaces and traits */
    private static function hierarchyNames(string $class): array
    {
        return array_values(array_unique(rescueReflection(fn () => [$class, ...@class_parents($class), ...@class_implements($class), ...self::traitsOf($class)]) ?? [$class]));
    }

    /** @return list<string> absolute paths of the class, its ancestors, interfaces and traits */
    public static function hierarchyFiles(string $class): array
    {
        if (! rescueReflection(fn () => class_exists($class) || interface_exists($class) || trait_exists($class))) {
            return [];
        }
        $names = self::hierarchyNames($class);
        $files = [];
        foreach ($names as $name) {
            $file = rescueReflection(fn () => (new ReflectionClass($name))->getFileName());
            if (is_string($file)) {
                $files[$file] = true;
            }
        }

        return array_keys($files);
    }

    private static function traitsOf(string $class): array
    {
        $traits = [];
        foreach ([$class, ...class_parents($class)] as $owner) {
            $queue = class_uses($owner);
            while ($queue !== []) {
                $trait = array_pop($queue);
                if (! isset($traits[$trait])) {
                    $traits[$trait] = true;
                    array_push($queue, ...array_values(class_uses($trait)));
                }
            }
        }

        return array_keys($traits);
    }

    /**
     * methods: lowercase name => [declaring class, name, file, line]
     *
     * @return array{name: string, file: ?string, line: int, ancestors: list<string>, methods: array<string, array>, magic: bool, deps: list<string>}|null
     */
    public function classFacts(string $class): ?array
    {
        $reflection = rescueReflection(fn () => new ReflectionClass($class));
        if (! $reflection instanceof ReflectionClass) {
            return null;
        }
        // Only methods this class declares itself (trait methods included); inherited ones are looked up on the
        // ancestors' own facts, which keeps a model class from repeating the hundreds of methods of its base.
        $methods = [];
        foreach ($reflection->getMethods() as $method) {
            if ($method->getDeclaringClass()->getName() !== $reflection->getName()) {
                continue;
            }
            $file = $this->files->relativePath((string) $method->getFileName());
            $methods[strtolower($method->getName())] = [
                $method->getDeclaringClass()->getName(),
                $method->getName(),
                $file,
                (int) $method->getStartLine(),
                $this->scopeSymbolAt($file, (int) $method->getStartLine(), $method->getName()),
                $method->isAbstract(),
            ];
        }
        $units = $this->concreteUnits(["class:{$reflection->getName()}" => true]);

        return [
            'name' => $reflection->getName(),
            'file' => $this->files->relativePath((string) $reflection->getFileName()),
            'line' => (int) $reflection->getStartLine(),
            'ancestors' => array_values([...class_parents($reflection->getName()), ...class_implements($reflection->getName()), ...self::traitsOf($reflection->getName())]),
            'trait' => $reflection->isTrait(),
            'methods' => $methods,
            'magic' => $reflection->hasMethod('__call') || $reflection->hasMethod('__callStatic'),
            'units' => $units,
            'deps' => self::unitPaths($units),
        ];
    }

    /** The symbol the source file declares at $line for $method (for a trait method: `Trait::method`). */
    private function scopeSymbolAt(?string $path, int $line, string $method): ?string
    {
        foreach (($path === null ? null : $this->files->get($path))?->scopes ?? [] as $scope) {
            if ($scope->kind === 'function' && strcasecmp((string) $scope->functionName, $method) === 0 && $line >= $scope->startLine - 50 && $line <= $scope->startLine) {
                return $scope->symbol();
            }
        }

        return null;
    }

    /** @return array{symbol: string, file: ?string, line: int}|null */
    public function functionFacts(string $name): ?array
    {
        if (! function_exists($name)) {
            return null;
        }
        $function = new ReflectionFunction($name);

        return ['symbol' => $function->getName(), 'file' => $this->files->relativePath((string) $function->getFileName()), 'line' => (int) $function->getStartLine()];
    }
}

/**
 * The facts of a file before its latest change, and the rules for which of them still hold after it.
 */
final class CarriedFacts
{
    /** @var array<string, int> old function symbol => old scope index (only symbols declared once) */
    private array $oldScopeBySymbol = [];

    /** @var array<string, array> question => [question, answer hash, units] */
    private array $oldRecords = [];

    private bool $headUnchanged;

    /** @param list<string> $changedPaths */
    public function __construct(private readonly array $previous, private readonly array $newUnits, private readonly string $path, array $changedPaths)
    {
        $this->headUnchanged = ($previous['units']['head'] ?? null) === ($newUnits['head'] ?? null);
        $counts = array_count_values(array_column($previous['scopes'], 0));
        foreach ($previous['scopes'] as $index => $scope) {
            if (! $scope[3] && $counts[$scope[0]] === 1) {
                $this->oldScopeBySymbol[$scope[0]] = $index;
            }
        }
        foreach ($previous['questions'] as $record) {
            $this->oldRecords[$record[0]] = $record;
        }
        $this->otherChangedPaths = array_flip(array_diff($changedPaths, [$path]));
    }

    /** @var array<string, int> */
    private array $otherChangedPaths;

    /** A unit is unchanged if it lives in this file with the same fingerprint, or in a file not changed now. */
    private function unitUnchanged(string $unit): bool
    {
        if (! str_contains($unit, '#')) {
            return false;
        }
        [$path, $key] = explode('#', $unit, 2);
        if ($path === $this->path) {
            return isset($this->previous['units'][$key]) && ($this->newUnits[$key] ?? null) === $this->previous['units'][$key];
        }

        return ! isset($this->otherChangedPaths[$path]);
    }

    /** @param list<string> $questions */
    private function recordsStillValid(array $questions): ?array
    {
        $records = [];
        foreach ($questions as $question) {
            $record = $this->oldRecords[$question] ?? null;
            if ($record === null) {
                return null;
            }
            foreach (array_keys($record[2]) as $unit) {
                if (! $this->unitUnchanged($unit)) {
                    return null;
                }
            }
            $records[] = $record;
        }

        return $records;
    }

    /** @return array{0: list<string>, 1: bool, 2: array}|null the old ancestry answer, if it still holds */
    public function ancestry(PhpScope $scope, string $class): ?array
    {
        if ($scope->kind !== 'class' || ! $this->headUnchanged) {
            return null;
        }
        $old = $this->previous['classes'][strtolower($class)] ?? null;
        $records = $this->recordsStillValid(["ancestry|{$class}"]);
        if ($old === null || $records === null) {
            return null;
        }
        $isTest = in_array('phpunit\framework\testcase', $old[1], true);

        return [$old[1], $isTest, $records[0]];
    }

    /**
     * Old call sites that still hold, keyed by their new token index; a null entry marks a site to skip
     * (none). Sites not listed are inferred again.
     *
     * @return array<int, array{0: array, 1: list<array>}|null>
     */
    public function sitesFor(PhpFileModel $file): array
    {
        if (! $this->headUnchanged) {
            return [];
        }
        $moves = [];
        foreach ($file->scopes as $newIndex => $scope) {
            $symbol = $scope->symbol();
            $oldIndex = $this->oldScopeBySymbol[$symbol] ?? null;
            if ($scope->kind !== 'function' || $oldIndex === null) {
                continue;
            }
            $bodyKey = 'body:'.strtolower($symbol);
            $shapeKey = $scope->className === null ? null : 'shape:'.strtolower($scope->className);
            $bodyUnchanged = isset($this->previous['units'][$bodyKey]) && $this->previous['units'][$bodyKey] === ($this->newUnits[$bodyKey] ?? null);
            $shapeUnchanged = $shapeKey === null || (isset($this->previous['units'][$shapeKey]) && $this->previous['units'][$shapeKey] === ($this->newUnits[$shapeKey] ?? null));
            if ($bodyUnchanged && $shapeUnchanged) {
                $old = $this->previous['scopes'][$oldIndex];
                if (! isset($old[5])) {
                    continue;
                }
                $moves[$oldIndex] = [$newIndex, $scope->startLine - $old[1], $scope->bodyToken - $old[5]];
            }
        }
        $carried = [];
        foreach ($this->previous['byName'] as $sites) {
            foreach ($sites as $site) {
                $move = $moves[$site[5]] ?? null;
                if ($move === null) {
                    continue;
                }
                $questions = array_map(fn (int $position) => $this->previous['questions'][$position][0], $site[7]);
                $records = $this->recordsStillValid($questions);
                if ($records === null) {
                    continue;
                }
                [$newIndex, $lineShift, $tokenShift] = $move;
                $carried[$site[6] + $tokenShift] = [[$site[0], $site[1] + $lineShift, $site[2], $site[3], $site[4], $newIndex, $site[6] + $tokenShift], $records];
            }
        }

        return $carried;
    }
}
