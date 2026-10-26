<?php

declare(strict_types=1);

/**
 * Works out which classes an expression can hold, from what the code declares:
 * typed properties and parameters, return types, `new`, and @var/@return docblocks, read through Reflection.
 * Anything it cannot prove returns null, so the caller reports it as unresolved instead of guessing.
 */
final class TypeInference
{
    private const MAX_DEPTH = 8;

    /** @var array<string, array|null> lowercase `Class::method` => types its return statements produce */
    private array $returnTypes = [];

    /** @var array<string, array<string, true>> lowercase `Class::method` => code units its return statements relied on */
    private array $returnTypeUnits = [];

    /**
     * Code units the questions being answered relied on, innermost question last; the first frame collects
     * everything for the file being read. A unit is `class:<name>` (its declaration and ancestry),
     * `body:<path>|<symbol>` (a function body) or `file:<absolute path>` (a function's file).
     *
     * @var list<array<string, true>>
     */
    private array $unitFrames = [[]];

    /** @var array<string, true> questions answered since the caller last reset it (one call site at a time) */
    public array $answered = [];

    /**
     * Every question put to the engine (a return type, a property type, a function) with the answer it gave
     * and the code units that answer relied on. Asking again only the questions whose units changed tells
     * whether facts built on them are still true.
     *
     * @var array<string, array{0: mixed, 1: array<string, true>}>
     */
    public array $questions = [];

    public function __construct(private readonly PhpFileCache $files) {}

    /** @return array{types: list<string>, confidence: string}|null */
    public function expressionTypes(PhpFileModel $file, int $end, int $depth = 0): ?array
    {
        $token = $file->tokens[$end] ?? null;
        if ($token === null || $depth > self::MAX_DEPTH) {
            return null;
        }
        $previous = $file->tokens[$end - 1] ?? null;

        if ($token->id === T_VARIABLE) {
            return $token->text === '$this'
                ? self::known([$file->classAtToken($end)], 'exact')
                : $this->variableTypes($file, $token->text, $end, $depth);
        }

        if ($token->text === ')') {
            return $this->callResultTypes($file, $end, $depth);
        }

        if (isIdentifierText($token->text) || isNameToken($token)) {
            if ($previous?->text === '->' || $previous?->text === '?->') {
                return $this->propertyTypes($this->expressionTypes($file, $end - 2, $depth), $token->text);
            }
            if ($previous?->id === T_NEW) {
                return self::known([$file->resolveClassName($token->text, $file->classAtToken($end))], 'exact');
            }
        }
        // `$class = Foo::class; $class::make()`: the class the string names.
        if ($token->id === T_CLASS && $previous?->text === '::') {
            return $this->staticReceiverTypes($file, $end - 2, $depth);
        }

        return null;
    }

    /** Receiver of `X::method()`: a class name (`self`, `static`, `parent` included) or an expression like `$x::`. */
    public function staticReceiverTypes(PhpFileModel $file, int $end, int $depth = 0): ?array
    {
        $token = $file->tokens[$end] ?? null;
        if ($token === null) {
            return null;
        }
        if ($token->id === T_VARIABLE || $token->text === ')') {
            return $this->expressionTypes($file, $end, $depth);
        }
        if (isNameToken($token) || isIdentifierText($token->text)) {
            return self::known([$file->resolveClassName($token->text, $file->classAtToken($end))], 'exact');
        }

        return null;
    }

    private function callResultTypes(PhpFileModel $file, int $closeIndex, int $depth): ?array
    {
        $open = $file->matchingOpen($closeIndex);
        if ($open === null) {
            return null;
        }
        $callee = $file->tokens[$open - 1] ?? null;
        $operator = $file->tokens[$open - 2] ?? null;
        $currentClass = $file->classAtToken($open);

        // A keyword before the parenthesis (`return (new X)->...`) is no callee: the group's own content decides.
        $calleeIsName = $callee !== null && (isNameToken($callee) || (isIdentifierText($callee->text) && in_array($operator?->text, ['->', '?->', '::'], true)));
        if ($calleeIsName) {
            if ($operator?->text === '->' || $operator?->text === '?->') {
                return $this->methodReturnTypes($this->expressionTypes($file, $open - 3, $depth), $callee->text, false, $depth);
            }
            if ($operator?->text === '::') {
                return $this->methodReturnTypes($this->staticReceiverTypes($file, $open - 3, $depth), $callee->text, true, $depth);
            }
            if ($operator?->id === T_NEW) {
                return self::known([$file->resolveClassName($callee->text, $currentClass)], 'exact');
            }

            return $this->functionResultTypes($file, $callee->text, $open, $currentClass);
        }

        $first = $file->tokens[$open + 1] ?? null;
        if ($first?->id === T_NEW && isset($file->tokens[$open + 2])) {
            return self::known([$file->resolveClassName($file->tokens[$open + 2]->text, $currentClass)], 'exact');
        }

        return $this->expressionTypes($file, $closeIndex - 1, $depth + 1);
    }

    /** Declared return type of a function; otherwise a service-locator call like `factory(X::class)` probably yields X. */
    private function functionResultTypes(PhpFileModel $file, string $name, int $open, ?string $currentClass): ?array
    {
        $candidates = str_contains($name, '\\') ? [ltrim($name, '\\')] : [$file->namespace === '' ? $name : "{$file->namespace}\\{$name}", $name];
        $declared = $this->functionReturn($candidates);
        if ($declared !== []) {
            return self::known($declared, 'exact');
        }
        $argument = $file->tokens[$open + 1] ?? null;
        if (isNameToken($argument) && ($file->tokens[$open + 2]->text ?? '') === '::' && ($file->tokens[$open + 3]->id ?? null) === T_CLASS) {
            return self::known([$file->resolveClassName($argument->text, $currentClass)], 'likely');
        }

        return null;
    }

    /** @param list<string> $candidates names PHP tries in order, namespaced first */
    public function functionReturn(array $candidates): array
    {
        $this->beginQuestion();
        $declared = [];
        foreach ($candidates as $candidate) {
            if (function_exists($candidate)) {
                $function = new ReflectionFunction($candidate);
                $this->noteUnit('file:'.$function->getFileName());
                $declared = $this->typeNames($function->getReturnType(), null);
                break;
            }
        }

        return $this->answer('function|'.implode('|', $candidates), $declared);
    }

    public function noteUnit(string $unit): void
    {
        $this->unitFrames[count($this->unitFrames) - 1][$unit] = true;
    }

    public function beginQuestion(): void
    {
        $this->unitFrames[] = [];
    }

    /** Records the answer with the units noted since beginQuestion(), and passes those units on to the asker. */
    public function answer(string $question, mixed $answer): mixed
    {
        $units = count($this->unitFrames) > 1 ? array_pop($this->unitFrames) : [];
        $this->unitFrames[count($this->unitFrames) - 1] += $units;
        $this->questions[$question] = [$answer, $units];
        $this->answered[$question] = true;

        return $answer;
    }

    /** Units relied on by everything since the last call, for the file being read. */
    public function takeUnits(): array
    {
        $units = $this->unitFrames[0];
        $this->unitFrames = [[]];

        return $units;
    }

    private function methodReturnTypes(?array $receiver, string $method, bool $isStatic, int $depth): ?array
    {
        if ($receiver === null) {
            return null;
        }
        $types = [];
        $confidence = $receiver['confidence'];
        foreach ($receiver['types'] as $class) {
            [$declared, $certain] = $this->memberReturn($class, $method, $isStatic, $depth);
            array_push($types, ...$declared);
            if (! $certain) {
                $confidence = 'likely';
            }
        }

        return self::known($types, $confidence);
    }

    /** @return array{0: list<string>, 1: bool} what `$class::$method()` returns, and whether that is certain */
    public function memberReturn(string $class, string $method, bool $isStatic, int $depth = 0): array
    {
        $this->beginQuestion();
        $this->noteUnit("class:{$class}");
        $answer = [[], true];
        $reflection = rescueReflection(fn () => new ReflectionMethod($class, $method));
        if ($reflection instanceof ReflectionMethod) {
            $declared = $this->typeNames($reflection->getReturnType(), $class);
            if ($declared === []) {
                $declared = $this->docblockTypes($reflection->getDocComment(), '@return', null, $reflection->getDeclaringClass()->getName(), $class);
            }
            if ($declared === []) {
                $returned = $this->returnStatementTypes($reflection, $class, $depth);
                $answer = [$returned['types'] ?? [], $returned === null || $returned['confidence'] === 'exact'];
            } else {
                $answer = [$declared, true];
            }
        } elseif ($isStatic && rescueReflection(fn () => method_exists($class, '__callStatic'))) {
            // Static magic such as an Eloquent `Order::find()` usually returns the model itself.
            $answer = [[$class], false];
        }

        return $this->answer('return|'.$class.'|'.$method.'|'.($isStatic ? 'static' : 'instance'), $answer);
    }

    /** Undeclared return type: what the method's own `return` statements produce (`$this`, `new static`, ...). */
    private function returnStatementTypes(ReflectionMethod $reflection, string $selfClass, int $depth): ?array
    {
        $key = strtolower("{$selfClass}::{$reflection->getName()}");
        if (array_key_exists($key, $this->returnTypes)) {
            foreach (array_keys($this->returnTypeUnits[$key] ?? []) as $unit) {
                $this->noteUnit($unit);
            }

            return $this->returnTypes[$key];
        }
        $this->returnTypes[$key] = null;
        $this->unitFrames[] = [];
        $result = $this->returnStatementTypesUncached($reflection, $selfClass, $depth);
        $units = array_pop($this->unitFrames);
        $this->unitFrames[count($this->unitFrames) - 1] += $units;
        $this->returnTypeUnits[$key] = $units;

        return $this->returnTypes[$key] = $result;
    }

    private function returnStatementTypesUncached(ReflectionMethod $reflection, string $selfClass, int $depth): ?array
    {
        $path = $this->files->relativePath((string) $reflection->getFileName());
        $file = $path !== null ? $this->files->get($path) : null;
        $scope = null;
        foreach ($file?->scopes ?? [] as $candidate) {
            if ($candidate->kind === 'function' && strcasecmp((string) $candidate->functionName, $reflection->getName()) === 0
                && $candidate->startLine >= $reflection->getStartLine() && $candidate->startLine <= $reflection->getEndLine()) {
                $scope = $candidate;
                break;
            }
        }
        if ($scope === null) {
            return null;
        }
        $this->noteUnit("body:{$path}|".strtolower($scope->symbol()));

        $types = [];
        $allResolved = true;
        $declaringClass = $file->classAtToken($scope->startToken);
        for ($index = $scope->startToken + 1; $index < $scope->endToken; $index++) {
            $token = $file->tokens[$index];
            if ($token->id === T_FUNCTION) {
                $index = $this->closureEnd($file, $index);
            } elseif ($token->id === T_RETURN && $file->tokens[$index + 1]->text !== ';') {
                $returned = $this->expressionTypes($file, $file->expressionEnd($index + 1) - 1, $depth + 1);
                $allResolved = $allResolved && $returned !== null && $returned['confidence'] === 'exact';
                foreach ($returned['types'] ?? [] as $type) {
                    $types[] = $type === $declaringClass ? $selfClass : $type;
                }
            }
        }

        return self::known($types, $allResolved ? 'exact' : 'likely');
    }

    /** Index of the `}` closing the closure whose `function` keyword is at $functionIndex. */
    private function closureEnd(PhpFileModel $file, int $functionIndex): int
    {
        $depth = 0;
        $count = count($file->tokens);
        for ($index = $functionIndex; $index < $count; $index++) {
            $text = $file->tokens[$index]->text;
            if ($text === '{') {
                $depth++;
            } elseif ($text === '}' && --$depth === 0) {
                return $index;
            }
        }

        return $count;
    }

    private function propertyTypes(?array $receiver, string $property): ?array
    {
        if ($receiver === null) {
            return null;
        }
        $types = [];
        foreach ($receiver['types'] as $class) {
            array_push($types, ...$this->memberProperty($class, $property));
        }

        return self::known($types, $receiver['confidence']);
    }

    /** @return list<string> declared (or documented) types of `$class->$property` */
    /** An untyped property the constructor sets from a typed parameter: `$this->mailer = $mailer;` with `Mailer $mailer`. */
    private function constructorAssigned(string $declaringClass, string $property, string $selfClass): array
    {
        $constructor = rescueReflection(fn () => (new ReflectionClass($declaringClass))->getConstructor());
        $file = $constructor instanceof ReflectionMethod && $constructor->getDeclaringClass()->getName() === $declaringClass ? $this->files->forClass($declaringClass) : null;
        $scope = null;
        foreach ($file?->scopes ?? [] as $candidate) {
            if ($candidate->kind === 'function' && strcasecmp((string) $candidate->functionName, '__construct') === 0 && strcasecmp((string) $candidate->className, $declaringClass) === 0) {
                $scope = $candidate;
            }
        }
        if ($scope === null) {
            return [];
        }
        $this->noteUnit("body:{$file->path}|".strtolower($scope->symbol()));
        for ($index = $scope->bodyToken; $index < $scope->endToken; $index++) {
            $tokens = $file->tokens;
            $assigns = $tokens[$index]->text === '$this' && ($tokens[$index + 1]->text ?? '') === '->' && ($tokens[$index + 2]->text ?? '') === $property
                && ($tokens[$index + 3]->text ?? '') === '=' && ($tokens[$index + 4]->id ?? null) === T_VARIABLE && in_array($tokens[$index + 5]->text ?? '', [';', '??'], true);
            if (! $assigns) {
                continue;
            }
            foreach ($constructor->getParameters() as $parameter) {
                if ('$'.$parameter->getName() === $tokens[$index + 4]->text) {
                    return $this->typeNames($parameter->getType(), $selfClass);
                }
            }
        }

        return [];
    }

    public function memberProperty(string $class, string $property): array
    {
        $this->beginQuestion();
        $this->noteUnit("class:{$class}");
        $declared = [];
        $reflection = rescueReflection(fn () => (new ReflectionClass($class))->hasProperty($property) ? new ReflectionProperty($class, $property) : null);
        if ($reflection instanceof ReflectionProperty) {
            $declared = $this->typeNames($reflection->getType(), $class);
            if ($declared === []) {
                $declared = $this->docblockTypes($reflection->getDocComment(), '@var', null, $reflection->getDeclaringClass()->getName(), $class);
            }
            if ($declared === []) {
                $declared = $this->constructorAssigned($reflection->getDeclaringClass()->getName(), $property, $class);
            }
        }

        return $this->answer('property|'.$class.'|'.$property, $declared);
    }

    /** Asks a recorded question again, on the code as it is now. */
    public function ask(string $question): mixed
    {
        $parts = explode('|', $question);

        return match ($parts[0]) {
            'return' => $this->memberReturn($parts[1], $parts[2], $parts[3] === 'static'),
            'property' => $this->memberProperty($parts[1], $parts[2]),
            'function' => $this->functionReturn(array_slice($parts, 1)),
        };
    }

    /**
     * Finds the closest earlier declaration of $variable in the enclosing function:
     * an assignment, a typed parameter or catch, or an inline `@var` docblock.
     */
    private function variableTypes(PhpFileModel $file, string $variable, int $useIndex, int $depth): ?array
    {
        $scope = $file->innermostScopeAtToken($useIndex, 'function');
        $start = $scope?->startToken ?? 0;

        for ($index = $useIndex - 1; $index >= $start; $index--) {
            $token = $file->tokens[$index];
            if ($token->id !== T_VARIABLE || $token->text !== $variable) {
                continue;
            }
            $next = $file->tokens[$index + 1] ?? null;
            $previous = $file->tokens[$index - 1] ?? null;
            if ($next?->text === '=') {
                $assigned = $this->rangeTypes($file, $index + 2, $file->expressionEnd($index + 2), $depth + 1);

                return $assigned === null || $assigned['types'] === [] ? null : $assigned;
            }
            if (isNameToken($previous) && ! in_array($file->tokens[$index - 2]->text ?? '', ['->', '?->', '::'], true)) {
                return self::known($this->declaredTypesBefore($file, $index), 'exact');
            }
        }

        return $this->inlineVarDocblock($file, $variable, $scope?->startLine ?? 1, $file->tokens[$useIndex]->line);
    }

    /**
     * Types of the expression in tokens [$start, $end): each branch of `c ? a : b`, `a ?: b` and `a ?? b` counts,
     * a `null` branch adds nothing, and one branch of unknown type makes the whole unknown.
     */
    private function rangeTypes(PhpFileModel $file, int $start, int $end, int $depth): ?array
    {
        $nesting = 0;
        $question = null;
        $pending = 0;
        $coalesce = null;
        for ($index = $start; $index < $end; $index++) {
            $token = $file->tokens[$index];
            if (in_array($token->text, ['(', '[', '{'], true) || $token->id === T_CURLY_OPEN) {
                $nesting++;
            } elseif (in_array($token->text, [')', ']', '}'], true)) {
                $nesting--;
            } elseif ($nesting !== 0) {
                continue;
            } elseif ($token->id === T_COALESCE && $question === null) {
                $coalesce ??= $index;
            } elseif ($token->text === '?') {
                $question ??= $index;
                $pending++;
            } elseif ($token->text === ':' && $question !== null && --$pending === 0) {
                $then = $index === $question + 1 ? $this->rangeTypes($file, $start, $question, $depth) : $this->rangeTypes($file, $question + 1, $index, $depth);

                return $this->branchTypes([$then, $this->rangeTypes($file, $index + 1, $end, $depth)]);
            }
        }
        // `??` binds tighter than `? :`, so it splits only an expression with no ternary at this level.
        if ($coalesce !== null) {
            return $this->branchTypes([$this->rangeTypes($file, $start, $coalesce, $depth), $this->rangeTypes($file, $coalesce + 1, $end, $depth)]);
        }
        if ($end - $start === 1 && in_array(strtolower($file->tokens[$start]->text), ['null', 'false'], true)) {
            return ['types' => [], 'confidence' => 'exact'];
        }

        return $this->expressionTypes($file, $end - 1, $depth);
    }

    /** @param list<array{types: list<string>, confidence: string}|null> $branches */
    private function branchTypes(array $branches): ?array
    {
        $types = [];
        $confidence = 'exact';
        foreach ($branches as $branch) {
            if ($branch === null) {
                return null;
            }
            array_push($types, ...$branch['types']);
            $confidence = $branch['confidence'] === 'exact' ? $confidence : $branch['confidence'];
        }

        return self::known($types, $confidence);
    }

    /** `A|?B $variable` written right before the variable. */
    private function declaredTypesBefore(PhpFileModel $file, int $variableIndex): array
    {
        $currentClass = $file->classAtToken($variableIndex);
        $types = [];
        for ($index = $variableIndex - 1; $index >= 0; $index--) {
            $token = $file->tokens[$index];
            if (isNameToken($token)) {
                $types[] = $file->resolveClassName($token->text, $currentClass);
            } elseif (! in_array($token->text, ['|', '?', '&'], true)) {
                break;
            }
        }

        return $types;
    }

    private function inlineVarDocblock(PhpFileModel $file, string $variable, int $fromLine, int $toLine): ?array
    {
        $pattern = '/@var\s+(\S+)\s+'.preg_quote($variable, '/').'\b/';
        for ($line = $toLine; $line >= $fromLine; $line--) {
            if (preg_match($pattern, $file->lines[$line - 1] ?? '', $match)) {
                $currentClass = $file->innermostScopeAtLine($line, 'class')?->className;

                return self::known($this->splitDocType($match[1], $file, $currentClass), 'exact');
            }
        }

        return null;
    }

    /** @return list<string> */
    private function docblockTypes(string|false $docComment, string $tag, ?PhpFileModel $file, string $declaringClass, string $selfClass): array
    {
        if ($docComment === false || ! preg_match('/'.preg_quote($tag, '/').'\s+(\S+)/', $docComment, $match)) {
            return [];
        }
        $file ??= $this->files->forClass($declaringClass);
        if ($file === null) {
            return [];
        }

        return $this->splitDocType($match[1], $file, $declaringClass, $selfClass);
    }

    /**
     * `?Foo|Bar<int, X>|null|Baz[]` → resolved class names, ignoring generics and arrays. `$this`, `static` and
     * `self` in a method's docblock mean the class it is called on ($selfClass), not the class declaring it:
     * `@return $this` on Eloquent's `forceFill()` keeps `$order->forceFill([...])->save()` on the order's class.
     */
    private function splitDocType(string $docType, PhpFileModel $file, ?string $currentClass, ?string $selfClass = null): array
    {
        $withoutGenerics = preg_replace('/<.*$/', '', $docType);
        $types = [];
        foreach (explode('|', $withoutGenerics) as $part) {
            $part = ltrim(trim($part), '?');
            if ($part === '' || str_ends_with($part, '[]')) {
                continue;
            }
            if ($selfClass !== null && in_array(strtolower($part), ['static', 'self', '$this'], true)) {
                $types[] = $selfClass;

                continue;
            }
            $types[] = $file->resolveClassName($part, $currentClass);
        }

        return array_values(array_filter($types));
    }

    /** @return list<string> */
    private function typeNames(?ReflectionType $type, ?string $selfClass): array
    {
        if ($type instanceof ReflectionNamedType) {
            $name = $type->getName();
            if (in_array(strtolower($name), ['static', 'self'], true)) {
                return $selfClass !== null ? [$selfClass] : [];
            }

            return $type->isBuiltin() ? [] : [$name];
        }
        if ($type instanceof ReflectionUnionType || $type instanceof ReflectionIntersectionType) {
            return array_merge(...array_map(fn (ReflectionType $member) => $this->typeNames($member, $selfClass), $type->getTypes()));
        }

        return [];
    }

    /** @param list<string|null> $types */
    private static function known(array $types, string $confidence): ?array
    {
        $types = array_values(array_unique(array_filter($types)));

        return $types === [] ? null : ['types' => $types, 'confidence' => $confidence];
    }
}
