<?php

declare(strict_types=1);

require __DIR__.'/../php/protocol.php';
require __DIR__.'/../php/worker.php';
require __DIR__.'/../php/php-source.php';
require __DIR__.'/../php/fact-extraction.php';

/**
 * Wiring that only exists at runtime: routes, event listeners (observers included), queued jobs and artisan commands.
 * Read from the application booted in a forked child, so it matches what Laravel will actually call, and kept
 * until a change can alter it: a file boot read changed outside function bodies, a function that ran during boot
 * changed, a file appeared in a directory boot read from, or a non-PHP input (.env) changed.
 *
 * Which functions ran during boot is recorded in the background by booting once more with an engine hook on every
 * function of the files boot loaded (the opentelemetry extension). Until that record exists, every change to a
 * file boot read means booting again.
 */
final class LaravelWiring implements BackgroundWork
{
    private const RESTART_INPUTS = ['vendor/composer/installed.json'];

    private const BOOT_INPUTS_OUTSIDE_GIT = ['.env'];

    private const BUS_DISPATCHABLE = 'Illuminate\Foundation\Bus\Dispatchable';

    private const DISPATCHABLE_TRAITS = [self::BUS_DISPATCHABLE, 'Illuminate\Foundation\Events\Dispatchable'];

    private const DISPATCH_METHODS = ['dispatch', 'dispatchIf', 'dispatchUnless', 'dispatchSync', 'dispatchAfterResponse', 'broadcast'];

    /** @var array{routes: array, listeners: array, commands: array, deps: list<string>, dirs: list<string>}|null */
    private ?array $wiring = null;

    /**
     * Why the last boot failed (an app that reaches a database it cannot open while booting), and when: kept until a
     * file changes or for BOOT_RETRY_SECONDS, so one question does not boot once per symbol and a database that comes
     * back is seen.
     *
     * @var array{0: string, 1: float}|null
     */
    private ?array $bootFailure = null;

    private const BOOT_RETRY_SECONDS = 30;

    /** @var array<string, list<array>> symbol => caller edges, valid until the wiring or a class it used changes */
    private array $callersCache = [];

    /** Bumped on every boot, so a boot record that arrives after the next boot is ignored. */
    private int $generation = 0;

    /** @var array{units: array<string, array|null>, ran: array<string, true>}|null what ran during the current boot */
    private ?array $bootRecord = null;

    /** @var array{0: int, 1: resource, 2: string, 3: int}|null the background boot recording which functions ran */
    private ?array $recording = null;

    /** @var array<string, array{traits: list<string>, dispatchMethods: list<string>, exists: bool, deps: list<string>}> lowercase class => traits */
    private array $classTraits = [];

    public function __construct(
        private readonly PhpFileCache $files,
        private readonly EngineGuard $guard,
    ) {}

    /** @param list<string> $changed */
    public function sync(array $changed): array
    {
        $this->guard->apply($changed, self::RESTART_INPUTS);
        $this->files->changed($changed);
        if ($changed !== []) {
            $this->bootFailure = null;
        }
        $this->forgetScansTouching($changed);
        if ($this->traitJobs !== []) {
            array_push($this->changeHistory, ...$changed);
            $this->changedSoFar = count($this->changeHistory);
        } else {
            $this->changeHistory = [];
            $this->changedSoFar = 0;
        }
        if ($this->wiring !== null && $this->changeCanAlterWiring($changed)) {
            if (ForkedWorker::inline()) {
                // The app was booted in this process and cannot be booted twice in it.
                EngineGuard::restart('the wiring changed and this system cannot fork');
            }
            $this->wiring = null;
            $this->bootRecord = null;
            $this->callersCache = [];
        }
        foreach ($this->classTraits as $class => $facts) {
            if ($this->touches($changed, $facts['deps'], [])) {
                unset($this->classTraits[$class]);
                $this->callersCache = [];
            }
        }
        // Boot again now, while the other drivers handle the same change, rather than on the first question.
        $this->wiring();

        return ['watch' => [...self::RESTART_INPUTS, ...self::BOOT_INPUTS_OUTSIDE_GIT]];
    }

    /** @param list<string> $changed */
    private function changeCanAlterWiring(array $changed): bool
    {
        $deps = array_flip($this->wiring['deps']);
        $dirs = array_flip($this->wiring['dirs']);
        foreach ($changed as $path) {
            $isDependency = isset($deps[$path]);
            if (! $isDependency && ! isset($dirs[dirname($path)])) {
                continue;
            }
            // Boot never read this file: only its appearing or disappearing (directory discovery) can matter.
            if (! $isDependency && isset($this->wiring['listing'][$path]) === is_file($this->files->repoRoot.'/'.$path)) {
                continue;
            }
            if (! $isDependency || ! str_ends_with($path, '.php') || ! $this->onlyIdleBodiesChanged($path)) {
                profilePhase('wiring: must boot again', microtime(true), ['path' => $path, 'dependency' => $isDependency, 'recorded' => isset($this->bootRecord['units'][$path]), 'reason' => $this->lastReason]);

                return true;
            }
        }

        return false;
    }

    /** True when $path changed only inside bodies of functions that did not run during the current boot. */
    private function onlyIdleBodiesChanged(string $path): bool
    {
        $before = $this->bootRecord['units'][$path] ?? null;
        $source = @file_get_contents($this->files->repoRoot.'/'.$path);
        if ($before === null || $source === false) {
            $this->lastReason = $this->bootRecord === null ? 'no boot record yet' : 'file not in boot record';

            return false;
        }
        $after = (new PhpFileModel($path, $source))->unitHashes();
        if (($before['outside'] ?? null) !== ($after['outside'] ?? null) || ($before['head'] ?? null) !== ($after['head'] ?? null)) {
            $this->lastReason = 'changed outside function bodies';

            return false;
        }
        foreach (array_keys($before + $after) as $key) {
            if (str_starts_with($key, 'body:') && ($before[$key] ?? null) !== ($after[$key] ?? null) && isset($this->bootRecord['ran']["{$path}#{$key}"])) {
                $this->lastReason = "{$key} ran during boot";

                return false;
            }
        }
        $this->bootRecord['units'][$path] = $after;

        return true;
    }

    public function streams(): array
    {
        $streams = array_map(fn (array $job) => $job[1], array_values($this->traitJobs));

        return $this->recording === null ? $streams : [$this->recording[1], ...$streams];
    }

    /**
     * Describes the traits of every project class from Composer's classmap in the background, in batches; a batch
     * whose child dies (PHP aborts on some broken classes) is dropped and those classes are described on demand.
     */
    public function startTraitWarmUp(): void
    {
        $root = $this->files->repoRoot;
        foreach (require "{$root}/vendor/composer/autoload_classmap.php" as $class => $file) {
            $relative = $this->files->relativePath((string) realpath($file));
            if ($relative !== null && ! str_starts_with($relative, 'vendor/')) {
                $this->traitQueue[] = $class;
            }
        }
        $this->fillTraitWarmUp();
    }

    private function fillTraitWarmUp(): void
    {
        $files = $this->files;
        while (count($this->traitJobs) < 2 && $this->traitQueue !== []) {
            $chunk = array_splice($this->traitQueue, 0, 100);
            $job = ForkedWorker::startBackground(function () use ($chunk, $files) {
                ini_set('opcache.enable', '0');

                return array_map(fn (string $class) => self::describeTraits($class, $files), $chunk);
            }, 'warm-up traits', takesSlot: true);
            $this->traitJobs[(int) $job[1]] = [...$job, $chunk, $this->changedSoFar];
        }
    }

    public function onReadable($stream): void
    {
        $key = (int) $stream;
        if (isset($this->traitJobs[$key])) {
            $job = $this->traitJobs[$key];
            [$finished, $described] = ForkedWorker::readAvailable($job);
            $this->traitJobs[$key] = $job;
            if (! $finished) {
                return;
            }
            unset($this->traitJobs[$key]);
            $changedSince = array_slice($this->changeHistory, $job[4]);
            foreach (is_array($described) ? $job[3] : [] as $index => $class) {
                $facts = $described[$index];
                if (! isset($this->classTraits[strtolower($class)]) && array_intersect($facts['deps'], $changedSince) === []) {
                    $this->classTraits[strtolower($class)] = $facts;
                }
            }
            $this->fillTraitWarmUp();

            return;
        }
        [$finished, $record] = ForkedWorker::readAvailable($this->recording);
        if (! $finished) {
            return;
        }
        if ($this->recording[3] === $this->generation && is_array($record)) {
            $this->bootRecord = $record;
            profilePhase('wiring: boot record', $this->recordingStarted, ['files' => count($record['units']), 'ran' => count($record['ran'])]);
        }
        $this->recording = null;
    }

    private float $recordingStarted = 0.0;

    /** Why the last change was judged able to alter the wiring (for the profile). */
    private string $lastReason = '';

    /** @var list<string> classes whose traits are still to describe in the background */
    private array $traitQueue = [];

    /** @var array<int, array{0: int, 1: resource, 2: string, 3: list<string>, 4: int}> trait batches by stream id */
    private array $traitJobs = [];

    /** @var list<string> every path reported changed, in order, so a background batch can tell what changed after it started */
    private array $changeHistory = [];

    private int $changedSoFar = 0;

    public function probe(): array
    {
        return [
            'driver' => 'laravel',
            'laravel' => class_exists(\Illuminate\Foundation\Application::class) ? \Illuminate\Foundation\Application::VERSION : null,
            'bootHooks' => function_exists('OpenTelemetry\\Instrumentation\\hook'),
            'wiringBooted' => $this->wiring !== null,
            'bootRecorded' => $this->bootRecord !== null,
            'classTraitsKnown' => count($this->classTraits),
        ];
    }

    public function hasIdleWork(): bool
    {
        return false;
    }

    public function idleStep(): void {}

    /**
     * Boots once more in the background with a hook on every function of the files the last boot loaded, and
     * records which ran. Trait methods run under the using class's name, so they are taken as having run.
     */
    private function recordBoot(): void
    {
        // Without fork the record would boot the app a second time inside the driver; every change reboots instead.
        if (! function_exists('OpenTelemetry\Instrumentation\hook') || ForkedWorker::inline()) {
            return;
        }
        $root = $this->files->repoRoot;
        $files = $this->files;
        $deps = $this->wiring['deps'];
        $hashes = $this->wiring['hashes'];
        $this->recordingStarted = microtime(true);
        $job = ForkedWorker::startBackground(function () use ($root, $files, $deps, $hashes) {
            ini_set('opcache.enable', '0');
            $units = [];
            $ran = [];
            $targets = [];
            foreach ($deps as $path) {
                if (! str_ends_with($path, '.php')) {
                    continue;
                }
                $model = $files->get($path);
                if ($model === null || FactExtraction::contentHash("{$root}/{$path}") !== ($hashes[$path] ?? null)) {
                    continue;
                }
                $units[$path] = $model->unitHashes();
                $traits = $model->traitNames();
                foreach ($model->scopes as $scope) {
                    if ($scope->kind !== 'function') {
                        continue;
                    }
                    $unit = "{$path}#body:".strtolower($scope->symbol());
                    if ($scope->className !== null && isset($traits[strtolower($scope->className)])) {
                        $ran[$unit] = true;
                    } else {
                        $targets[strtolower(($scope->className ?? '').'::'.$scope->functionName)][] = $unit;
                    }
                }
            }
            $record = function ($object, array $parameters, ?string $class, string $function) use (&$ran, $targets): void {
                foreach ($targets[strtolower(($class ?? '').'::'.$function)] ?? [] as $unit) {
                    $ran[$unit] = true;
                }
            };
            foreach (array_keys($targets) as $target) {
                [$class, $function] = explode('::', $target, 2);
                OpenTelemetry\Instrumentation\hook($class === '' ? null : $class, $function, $record);
            }
            self::boot($root, $files);

            return ['units' => $units, 'ran' => $ran];
        }, 'boot record');
        $this->recording = [...$job, $this->generation];
    }

    private function touches(array $changed, array $deps, array $dirs): bool
    {
        return array_intersect($changed, $deps) !== [] || array_intersect(array_map('dirname', $changed), $dirs) !== [];
    }

    private function wiring(): array
    {
        if ($this->wiring === null) {
            if ($this->bootFailure !== null && microtime(true) - $this->bootFailure[1] < self::BOOT_RETRY_SECONDS) {
                throw new RuntimeException($this->bootFailure[0]);
            }
            $root = $this->files->repoRoot;
            $files = $this->files;
            $started = microtime(true);
            try {
                $this->wiring = ForkedWorker::run(fn () => self::boot($root, $files));
            } catch (RuntimeException $error) {
                $this->bootFailure = [$error->getMessage(), microtime(true)];

                throw $error;
            }
            profilePhase('wiring: boot', $started, ['deps' => count($this->wiring['deps'])]);
            $this->generation++;
            $this->recordBoot();
        }

        return $this->wiring;
    }

    private static function boot(string $root, PhpFileCache $files): array
    {
        ob_start();
        try {
            $started = microtime(true);
            $app = require $root.'/bootstrap/app.php';
            $kernel = $app->make(Illuminate\Contracts\Console\Kernel::class);
            profilePhase('boot: application', $started);
            foreach ((fn () => $this->bootstrappers())->call($kernel) as $bootstrapper) {
                $started = microtime(true);
                $app->bootstrapWith([$bootstrapper]);
                profilePhase('boot: '.substr(strrchr($bootstrapper, '\\'), 1), $started);
            }
            $started = microtime(true);
            $wiring = [
                'routes' => self::routes($app['router']->getRoutes()->getRoutes(), $files),
                'listeners' => self::listeners($app['events']->getRawListeners()),
                'commands' => [],
            ];
            profilePhase('boot: read routes + listeners', $started);
            $started = microtime(true);
            foreach ($kernel->all() as $name => $command) {
                $wiring['commands'][strtolower(get_class($command).'::handle')][] = $name;
            }
            profilePhase('boot: read commands', $started);
            $started = microtime(true);
            $wiring['middleware'] = self::middleware($app);
            $wiring['formRequests'] = self::formRequests($app['router']->getRoutes()->getRoutes());
            $wiring['facades'] = self::facades($app, $root);
            $wiring['policies'] = array_change_key_case(array_flip(array_map('strtolower', Illuminate\Support\Facades\Gate::policies())));
            $wiring['macros'] = self::macros();
            profilePhase('boot: read middleware, form requests, facades, policies', $started);
        } finally {
            ob_end_clean();
        }
        $started = microtime(true);
        $deps = [];
        foreach (get_included_files() as $file) {
            $relative = $files->relativePath($file);
            if ($relative !== null && ! str_starts_with($relative, 'vendor/')) {
                $deps[$relative] = true;
            }
        }
        $wiring['hashes'] = [];
        foreach (array_keys($deps) as $path) {
            if (str_ends_with($path, '.php')) {
                $wiring['hashes'][$path] = FactExtraction::contentHash("{$root}/{$path}");
            }
        }
        foreach (self::BOOT_INPUTS_OUTSIDE_GIT as $input) {
            $deps[$input] = true;
        }
        $wiring['deps'] = array_keys($deps);
        $wiring['dirs'] = array_values(array_unique(array_map('dirname', $wiring['deps'])));
        $wiring['listing'] = [];
        foreach ($wiring['dirs'] as $directory) {
            foreach (scandir("{$root}/{$directory}") ?: [] as $entry) {
                $wiring['listing'][$directory === '.' ? $entry : "{$directory}/{$entry}"] = true;
            }
        }
        profilePhase('boot: inputs (hashes, listing)', $started);

        return $wiring;
    }

    /** Lowercase class => lowercase names of the macros registered on it (`Builder::macro('whereActive', ...)`). */
    private static function macros(): array
    {
        $byClass = [];
        foreach (get_declared_classes() as $class) {
            if (! method_exists($class, 'hasMacro') || ! property_exists($class, 'macros')) {
                continue;
            }
            $names = rescueReflection(fn () => array_keys((new ReflectionProperty($class, 'macros'))->getValue()));
            if ($names) {
                $byClass[strtolower($class)] = array_map('strtolower', $names);
            }
        }

        return $byClass;
    }

    /** Middleware class => what runs it: the routes it guards, or every request for global middleware. */
    private static function middleware($app): array
    {
        $byClass = [];
        $router = $app['router'];
        foreach ($router->getRoutes()->getRoutes() as $route) {
            foreach (rescueReflection(fn () => $router->gatherRouteMiddleware($route)) ?? [] as $middleware) {
                if (is_string($middleware)) {
                    $byClass[strtolower(ltrim(strtok($middleware, ':'), '\\'))][] = self::routeLabel($route);
                }
            }
        }
        $http = rescueReflection(fn () => $app->make(Illuminate\Contracts\Http\Kernel::class));
        foreach ($http && method_exists($http, 'getGlobalMiddleware') ? $http->getGlobalMiddleware() : [] as $middleware) {
            if (is_string($middleware)) {
                $byClass[strtolower(ltrim($middleware, '\\'))][] = 'every HTTP request (global middleware)';
            }
        }

        return array_map(fn (array $labels) => array_values(array_unique($labels)), $byClass);
    }

    /** Form request class => the actions that take it: Laravel validates the request before running them. */
    private static function formRequests(array $routes): array
    {
        $byRequest = [];
        foreach ($routes as $route) {
            $action = $route->getActionName();
            if (! str_contains($action, '@')) {
                continue;
            }
            [$class, $method] = explode('@', $action, 2);
            foreach (rescueReflection(fn () => (new ReflectionMethod($class, $method))->getParameters()) ?? [] as $parameter) {
                $type = $parameter->getType();
                $name = $type instanceof ReflectionNamedType ? $type->getName() : null;
                if ($name !== null && rescueReflection(fn () => is_subclass_of($name, Illuminate\Foundation\Http\FormRequest::class))) {
                    $byRequest[strtolower($name)][] = ltrim($class, '\\')."::{$method}";
                }
            }
        }

        return array_map(fn (array $actions) => array_values(array_unique($actions)), $byRequest);
    }

    /**
     * Class a facade forwards to => the facades: its root object's class, and the classes its docblock names
     * with `@see` (`Cache` reaches `CacheManager`, which hands calls on to `Repository`).
     */
    private static function facades($app, string $root): array
    {
        $candidates = array_values(config('app.aliases') ?? []);
        foreach (require "{$root}/vendor/composer/autoload_classmap.php" as $class => $file) {
            if (str_contains($class, '\\Facades\\') && ! str_contains($file, '/vendor/')) {
                $candidates[] = $class;
            }
        }
        $byClass = [];
        foreach (array_unique($candidates) as $facade) {
            if (! rescueReflection(fn () => is_subclass_of($facade, Illuminate\Support\Facades\Facade::class))) {
                continue;
            }
            $targets = [];
            $object = rescueReflection(fn () => $facade::getFacadeRoot());
            if (is_object($object)) {
                $targets[] = get_class($object);
            }
            preg_match_all('/@see\s+\\\\?([\w\\\\]+)/', (string) (new ReflectionClass($facade))->getDocComment(), $seen);
            foreach ([...$targets, ...$seen[1]] as $target) {
                $byClass[strtolower(ltrim($target, '\\'))][] = $facade;
            }
        }

        return array_map(fn (array $facades) => array_values(array_unique($facades)), $byClass);
    }

    private static function routeLabel($route): string
    {
        $verbs = implode('|', array_diff($route->methods(), ['HEAD']));
        $name = $route->getName() ? " ({$route->getName()})" : '';

        return "route {$verbs} /".ltrim($route->uri(), '/').$name;
    }

    /** Routes by the action they run: `Class::method`, or for a closure the place it starts (`routes/web.php:215`). */
    private static function routes(array $routes, PhpFileCache $files): array
    {
        $byAction = [];
        foreach ($routes as $route) {
            $action = $route->getActionName();
            $entry = self::routeLabel($route);
            if ($action === 'Closure') {
                $closure = new ReflectionFunction($route->getAction('uses'));
                $path = $files->relativePath((string) $closure->getFileName());
                if ($path !== null) {
                    $byAction[strtolower("{$path}:{$closure->getStartLine()}")][] = $entry;
                }

                continue;
            }
            [$class, $method] = str_contains($action, '@') ? explode('@', $action, 2) : [$action, '__invoke'];
            $byAction[strtolower(ltrim($class, '\\')."::{$method}")][] = $entry;
        }

        return $byAction;
    }

    private static function listeners(array $rawListeners): array
    {
        $byListener = [];
        foreach ($rawListeners as $event => $listeners) {
            foreach ($listeners as $listener) {
                if (is_string($listener)) {
                    [$class, $method] = str_contains($listener, '@') ? explode('@', $listener, 2) : [$listener, null];
                } elseif (is_array($listener) && count($listener) === 2) {
                    [$class, $method] = [is_object($listener[0]) ? get_class($listener[0]) : $listener[0], $listener[1]];
                } else {
                    continue;
                }
                $method ??= method_exists($class, 'handle') ? 'handle' : '__invoke';
                $byListener[strtolower(ltrim($class, '\\')."::{$method}")][] = ['event' => $event, 'isClass' => class_exists($event)];
            }
        }

        return $byListener;
    }

    private function traitsOf(string $class): array
    {
        $key = strtolower($class);
        if (! isset($this->classTraits[$key])) {
            $files = $this->files;
            $this->classTraits[$key] = ForkedWorker::run(fn () => self::describeTraits($class, $files), 'traits');
        }

        return $this->classTraits[$key];
    }

    /** Traits and dispatch methods of $class, with the project files that declare it and its ancestors. */
    private static function describeTraits(string $class, PhpFileCache $files): array
    {
        $exists = (bool) rescueReflection(fn () => class_exists($class));
        $deps = [];
        foreach ($exists ? FactExtraction::hierarchyFiles($class) : [] as $file) {
            $relative = $files->relativePath($file);
            if ($relative !== null && ! str_starts_with($relative, 'vendor/')) {
                $deps[] = $relative;
            }
        }

        return [
            'exists' => $exists,
            'trait' => (bool) rescueReflection(fn () => trait_exists($class)),
            'traits' => $exists ? array_values(class_uses_recursive($class)) : [],
            'parents' => $exists ? array_values(class_parents($class)) : [],
            'dispatchMethods' => $exists ? array_values(array_filter(self::DISPATCH_METHODS, fn (string $name) => method_exists($class, $name))) : [],
            'deps' => $deps,
        ];
    }

    /** @return list<array<string, mixed>> */
    public function callers(string $symbol): array
    {
        if (! isset($this->callersCache[$symbol])) {
            $started = microtime(true);
            $this->computing = $symbol;
            $this->callersCache[$symbol] = $this->computeCallers($symbol);
            $this->computing = null;
            profilePhase('laravel callers: computed', $started, ['symbol' => $symbol]);
        }

        return $this->callersCache[$symbol];
    }

    private function computeCallers(string $symbol): array
    {
        // Another language's symbol (`path#name`): no PHP name contains these.
        if (str_contains($symbol, '#')) {
            return [];
        }
        $key = strtolower($symbol);
        $wiring = $this->wiring();
        $edges = [];
        foreach (array_unique($wiring['routes'][$key] ?? []) as $route) {
            $edges[] = self::edge($route, 'entry', 'route');
        }
        // A closure at the top of a file (`routes/web.php:215`): its route, else the code it is written in (a group,
        // a callback inside a route), which runs it.
        if (str_contains($symbol, '/')) {
            return $edges !== [] ? $edges : $this->enclosingOf($symbol);
        }
        [$class, $method] = str_contains($symbol, '::') ? explode('::', $symbol, 2) : [$symbol, '__construct'];
        foreach ($wiring['listeners'][$key] ?? [] as $listened) {
            $edges[] = $listened['isClass']
                ? $this->located(self::edge("{$listened['event']}::__construct", null, 'event'))
                : self::edge("event {$listened['event']}", 'entry', 'event');
        }
        foreach ($wiring['commands'][$key] ?? [] as $command) {
            $edges[] = self::edge("artisan {$command}", 'entry', 'command');
            array_push($edges, ...$this->commandCallers($command));
        }

        $classKey = strtolower(ltrim($class, '\\'));
        $guarded = $wiring['middleware'][$classKey] ?? [];
        if ($guarded !== [] && in_array(strtolower($method), ['handle', 'terminate'], true)) {
            // One entry per route would bury the answer for a middleware on hundreds of routes.
            $labels = count($guarded) <= 3 ? $guarded : [sprintf('middleware on %d routes, e.g. %s', count($guarded), implode(', ', array_slice($guarded, 0, 2)))];
            foreach ($labels as $label) {
                $edges[] = self::edge(count($guarded) <= 3 && ! str_starts_with($label, 'every') ? "middleware of {$label}" : $label, 'entry', 'middleware');
            }
        }
        if (in_array(strtolower($method), self::FORM_REQUEST_HOOKS, true)) {
            foreach ($wiring['formRequests'][$classKey] ?? [] as $action) {
                $edges[] = $this->located(self::edge($action, null, 'validation'));
            }
        }
        foreach ($wiring['facades'][$classKey] ?? [] as $facade) {
            array_push($edges, ...$this->staticCallsThrough($facade, $method));
        }
        // A macro is called by its name on any object of the class, whose type is rarely known where it is called.
        if (in_array(strtolower($method), $wiring['macros'][$classKey] ?? [], true)) {
            array_push($edges, ...$this->scanTokens("macro|{$classKey}|{$method}", [$method], ['--word-regexp', '--fixed-strings', '-e', $method], function ($file, int $index) use ($method) {
                $tokens = $file->tokens;
                $called = strcasecmp($tokens[$index]->text, $method) === 0 && in_array($tokens[$index - 1]->text ?? '', ['->', '?->', '::'], true) && ($tokens[$index + 1]->text ?? '') === '(';

                return $called ? ['macro', 'unresolved'] : null;
            }));
        }
        // A policy method is an ability checked by name: `$this->authorize('update', $post)`, `$user->can(...)`.
        if (str_ends_with($classKey, 'policy')) {
            $model = $wiring['policies'][$classKey] ?? substr($class, 0, -strlen('Policy'));
            array_push($edges, ...$this->abilityChecks(strtolower(substr(strrchr('\\'.$model, '\\'), 1)), $method));
        }

        // Eloquent reaches some methods by a name derived from theirs: `scopeActive` as `->active()`,
        // `getFullNameAttribute` as `->full_name`.
        // One a trait declares is reached through each class using it (`Combo::query()->workspaceLevel()`).
        $owners = ($this->traitsOf($class)['trait'] ?? false) ? $this->classesUsing($class) : [$class];
        foreach ($owners as $owner) {
            if (preg_match('/^scope(\w+)$/', $method, $match)) {
                array_push($edges, ...$this->conventionUses($owner, lcfirst($match[1]), true));
            } elseif (preg_match('/^get(\w+)Attribute$/', $method, $match)) {
                array_push($edges, ...$this->conventionUses($owner, \Illuminate\Support\Str::snake($match[1]), false));
            }
        }

        // Methods the framework itself calls on classes of a kind: the entry point is the framework's step.
        foreach (self::FRAMEWORK_CALLS as [$parent, $methods, $entry]) {
            if (in_array(strtolower($method), $methods, true) && in_array($parent, $this->traitsOf($class)['parents'] ?? [], true)) {
                $edges[] = self::edge("{$entry} ({$class})", 'entry', 'framework');
            }
        }

        $isHandle = strcasecmp($method, 'handle') === 0;
        $isConstructor = strcasecmp($method, '__construct') === 0;
        if ($isHandle || $isConstructor) {
            $traits = $this->traitsOf($class);
            $dispatchable = array_intersect(self::DISPATCHABLE_TRAITS, $traits['traits']) !== [];
            if ($isHandle && in_array(self::BUS_DISPATCHABLE, $traits['traits'], true)) {
                $edges[] = $this->located(self::edge("{$class}::__construct", 'bridge', 'queue'));
            }
            if ($isConstructor && $dispatchable) {
                foreach ($traits['dispatchMethods'] as $dispatchMethod) {
                    $edges[] = $this->located(self::edge("{$class}::{$dispatchMethod}", 'bridge', 'dispatch'));
                }
            }
        }

        return $edges;
    }

    /**
     * Code that runs a command by its name: `Artisan::call('mail:sync')`, `$this->call(...)` in another command,
     * `Schedule::command('mail:sync')`. Found by the name written as a string; read from the files as they are now.
     */
    private function commandCallers(string $command): array
    {
        return $this->scanTokens("command|{$command}", [$command], ['-e', "['\"]".preg_quote($command)."[ '\"]"], function ($file, int $index) use ($command) {
            $tokens = $file->tokens;
            $callee = strtolower($tokens[$index - 2]->text ?? '');
            if ($tokens[$index]->id !== T_CONSTANT_ENCAPSED_STRING || ($tokens[$index - 1]->text ?? '') !== '(' || ! isset(self::COMMAND_RUNNERS[$callee])) {
                return null;
            }
            $written = substr($tokens[$index]->text, 1, -1);

            return $written === $command || str_starts_with($written, "{$command} ") ? [self::COMMAND_RUNNERS[$callee], 'exact'] : null;
        });
    }

    /**
     * The classes whose body has `use Trait;`, read from the files that name the trait.
     *
     * @return list<string>
     */
    private function classesUsing(string $trait): array
    {
        $short = strtolower(substr(strrchr('\\'.$trait, '\\'), 1));
        $uses = $this->scanTokens("uses|{$trait}", [$short], ['--word-regexp', '--fixed-strings', '--ignore-case', '-e', $short], function ($file, int $index) use ($short) {
            $tokens = $file->tokens;
            $name = strtolower(ltrim($tokens[$index]->text, '\\'));
            if ($name !== $short && ! str_ends_with($name, '\\'.$short)) {
                return null;
            }
            // `use A, Trait;` in a class body: names and commas back to `use` (a closure's `use (` has a parenthesis).
            $back = $index - 1;
            while ($back > 0 && ($tokens[$back]->text === ',' || in_array($tokens[$back]->id, [T_STRING, T_NAME_QUALIFIED, T_NAME_FULLY_QUALIFIED], true))) {
                $back--;
            }

            return $tokens[$back]->id === T_USE && $file->innermostScopeAtToken($index)?->kind === 'class' ? ['uses', 'exact'] : null;
        });

        return array_values(array_unique(array_map(fn (array $edge) => $edge['from']['symbol'], $uses)));
    }

    /**
     * `->name(` (a call) or `->name` (a read) in files that name the class: the object is rarely known (a relation,
     * a query), so these are `unresolved`; `Model::name(` names the class and is `likely`.
     */
    private function conventionUses(string $class, string $name, bool $call): array
    {
        $short = strtolower(substr(strrchr('\\'.$class, '\\'), 1));

        return $this->scanTokens("{$class}|{$name}|".($call ? 'call' : 'read'), [$name], ['--word-regexp', '--fixed-strings', '-e', $name], function ($file, int $index) use ($short, $name, $call) {
            $tokens = $file->tokens;
            $before = $tokens[$index - 1]->text ?? '';
            if ($tokens[$index]->text !== $name || ! in_array($before, ['->', '?->', '::'], true) || (($tokens[$index + 1]->text ?? '') === '(') !== $call || ! self::namesClass($file, $short)) {
                return null;
            }
            // `Post::active()` or `Post::query()->where(...)->active()`: the statement starts from the class itself.
            $named = false;
            for ($back = $index - 1; $back > 0 && ! in_array($tokens[$back]->text, [';', '{', '}'], true); $back--) {
                $named = $named || (strtolower($tokens[$back - 1]->text) === $short && $tokens[$back]->text === '::');
            }

            return [$call ? 'scope' : 'accessor', $named ? 'likely' : 'unresolved'];
        });
    }

    /** Whether a file writes the class's short name (imported, or qualified). */
    private static function namesClass(PhpFileModel $file, string $short): bool
    {
        foreach ($file->tokens as $token) {
            $text = strtolower($token->text);
            if ($text === $short || str_ends_with($text, "\\{$short}")) {
                return true;
            }
        }

        return false;
    }

    /** [parent class, lowercase methods, entry point]: what Laravel calls on its subclasses, and when. */
    private const FRAMEWORK_CALLS = [
        ['Illuminate\\Database\\Eloquent\\Model', ['boot', 'booted', 'booting'], 'Eloquent boots the model on first use'],
        ['Illuminate\\Support\\ServiceProvider', ['register', 'boot'], 'the application starts'],
        ['Illuminate\\Foundation\\Console\\Kernel', ['schedule', 'commands'], 'the scheduler and artisan start'],
        ['Illuminate\\Foundation\\Http\\Kernel', ['bootstrap'], 'an HTTP request starts'],
    ];

    /** The scope a file-level closure is written in (another closure, or the file's own code). */
    private function enclosingOf(string $symbol): array
    {
        [$path, $line] = array_pad(explode(':', $symbol, 2), 2, '');
        $file = $this->files->get($path);
        $closure = null;
        foreach ($file?->scopes ?? [] as $scope) {
            if ($scope->symbol() === $symbol) {
                $closure = $scope;
            }
        }
        if ($closure === null) {
            return [];
        }
        // Closures nested in file-level closures (`Route::group(function () { ... })`) all run when the file is
        // loaded: the file itself is what reaches them, not each group in turn.
        $outer = null;
        foreach ($file->scopes as $scope) {
            if ($scope !== $closure && ! $scope->isFileClosure && $scope->containsToken($closure->startToken) && ($outer === null || $scope->startToken > $outer->startToken)) {
                $outer = $scope;
            }
        }
        $from = $outer === null ? ['symbol' => $path, 'file' => $path, 'line' => 1, 'kind' => 'file'] : ['symbol' => $outer->symbol(), 'file' => $path, 'line' => $outer->startLine];

        return [['from' => $from, 'at' => ['file' => $path, 'line' => (int) $line], 'kind' => 'closure', 'confidence' => 'likely']];
    }

    /** Methods Laravel calls on a form request before the action that takes it runs. */
    private const FORM_REQUEST_HOOKS = ['authorize', 'rules', 'messages', 'attributes', 'prepareforvalidation', 'withvalidator', 'after', 'passedvalidation', 'failedvalidation', 'failedauthorization', 'validationdata'];

    /** Calls that check an ability by name, ability first. */
    private const ABILITY_CHECKS = ['authorize', 'can', 'cannot', 'cant', 'allows', 'denies', 'check', 'any', 'none', 'inspect'];

    /** `Cache::remember(...)` for the class the `Cache` facade forwards to: a call written through the facade. */
    private function staticCallsThrough(string $facade, string $method): array
    {
        $short = strtolower(substr(strrchr('\\'.$facade, '\\'), 1));

        return $this->scanTokens("facade|{$facade}|{$method}", [$method], ['--word-regexp', '--fixed-strings', '-e', $method], function ($file, int $index) use ($short, $method) {
            $tokens = $file->tokens;
            $written = strtolower($tokens[$index - 2]->text ?? '');
            $matches = strcasecmp($tokens[$index]->text, $method) === 0 && ($tokens[$index - 1]->text ?? '') === '::' && ($tokens[$index + 1]->text ?? '') === '('
                && ($written === $short || str_ends_with($written, "\\{$short}"));

            return $matches ? ['facade', 'likely'] : null;
        });
    }

    /** `authorize('update', ...)`, `can('update', ...)` in files that name the policy's model. */
    private function abilityChecks(string $modelShort, string $ability): array
    {
        return $this->scanTokens("ability|{$modelShort}|{$ability}", [$ability], ['--fixed-strings', '-e', "'{$ability}'", '-e', "\"{$ability}\""], function ($file, int $index) use ($modelShort, $ability) {
            $tokens = $file->tokens;
            $checked = $tokens[$index]->id === T_CONSTANT_ENCAPSED_STRING && substr($tokens[$index]->text, 1, -1) === $ability
                && ($tokens[$index - 1]->text ?? '') === '(' && in_array(strtolower($tokens[$index - 2]->text ?? ''), self::ABILITY_CHECKS, true);
            if (! $checked) {
                return null;
            }
            return self::namesClass($file, $modelShort) ? ['ability', 'unresolved'] : null;
        });
    }

    /**
     * Edges at every token a predicate accepts, in the files ripgrep finds. Kept until a file it matched, or a
     * file now containing one of `$words`, changes (see forgetScansTouching).
     *
     * @param  callable(PhpFileModel, int): ?array{0: string, 1: string}  $accepts  [edge kind, confidence] or null
     */
    private function scanTokens(string $cacheKey, array $words, array $ripgrep, callable $accepts): array
    {
        if ($this->computing !== null) {
            $this->scanUsers[$cacheKey][$this->computing] = true;
        }
        if (! isset($this->scans[$cacheKey])) {
            $edges = [];
            $paths = $this->files->ripgrep($ripgrep);
            foreach ($paths as $path) {
                $file = $this->files->get($path);
                foreach (array_keys($file?->tokens ?? []) as $index) {
                    [$kind, $confidence] = $accepts($file, $index) ?? [null, null];
                    if ($kind === null) {
                        continue;
                    }
                    $from = $file->symbolAtLine($file->tokens[$index]->line);
                    if ($this->files->isTestPath($path)) {
                        $from['kind'] = 'test';
                    }
                    $edges[] = ['from' => $from, 'at' => ['file' => $path, 'line' => $file->tokens[$index]->line], 'kind' => $kind, 'confidence' => $confidence];
                }
            }
            $this->scans[$cacheKey] = ['edges' => $edges, 'paths' => array_flip($paths), 'words' => $words];
        }

        return $this->scans[$cacheKey]['edges'];
    }

    /** Drops the scans a change can alter, and the caller lists built on them. */
    private function forgetScansTouching(array $changed): void
    {
        $sources = [];
        foreach ($changed as $path) {
            if (str_ends_with($path, '.php')) {
                $this->files->forget($path);
                $sources[$path] = strtolower((string) @file_get_contents("{$this->files->repoRoot}/{$path}"));
            }
        }
        foreach ($sources === [] ? [] : $this->scans as $key => $scan) {
            $touched = array_filter($sources, fn (string $source, string $path) => isset($scan['paths'][$path]) || array_filter($scan['words'], fn (string $word) => str_contains($source, strtolower($word))) !== [], ARRAY_FILTER_USE_BOTH) !== [];
            if ($touched) {
                unset($this->scans[$key]);
                foreach (array_keys($this->scanUsers[$key] ?? []) as $symbol) {
                    unset($this->callersCache[$symbol]);
                }
                unset($this->scanUsers[$key]);
            }
        }
    }

    /** Methods that run a command given by name, with the kind of edge each makes. */
    private const COMMAND_RUNNERS = ['call' => 'artisan', 'callsilent' => 'artisan', 'callsilently' => 'artisan', 'queue' => 'artisan', 'command' => 'schedule'];

    /** @var array<string, array{edges: list<array>, paths: array<string, int>, words: list<string>}> scanTokens() results */
    private array $scans = [];

    /** @var array<string, array<string, true>> scan key => symbols whose caller list used it */
    private array $scanUsers = [];

    /** The symbol whose callers are being computed, so the scans it uses know whom they serve. */
    private ?string $computing = null;

    /** Gives a `Class::method` edge the class's file and line, so a reader (and the grouped output) can find it. */
    private function located(array $edge): array
    {
        $class = explode('::', $edge['from']['symbol'], 2)[0];
        $file = composerClassFile($class);
        $relative = $file !== null ? $this->files->relativePath($file) : null;
        if ($relative === null) {
            return $edge;
        }
        $short = preg_quote(substr(strrchr('\\'.$class, '\\'), 1), '/');
        $line = 1;
        foreach (file($file) ?: [] as $index => $text) {
            if (preg_match('/\b(class|interface|trait|enum)\s+'.$short.'\b/', $text)) {
                $line = $index + 1;
                break;
            }
        }
        $edge['from'] += ['file' => $relative, 'line' => $line];

        return $edge;
    }

    private static function edge(string $symbol, ?string $kind, string $edgeKind): array
    {
        $from = ['symbol' => $symbol];
        if ($kind !== null) {
            $from['kind'] = $kind;
        }

        return ['from' => $from, 'kind' => $edgeKind, 'confidence' => 'exact'];
    }
}

$repoRoot = rtrim(realpath($argv[1] ?? '') ?: '', '/');
limitCompiledCode();
loadComposerAutoload($repoRoot);
$wiring = new LaravelWiring(new PhpFileCache($repoRoot), new EngineGuard($repoRoot));
$wiring->startTraitWarmUp();

serveDriver([
    'probe' => fn () => $wiring->probe(),
    'sync' => fn (array $request) => $wiring->sync($request['changed'] ?? []),
    'callers' => fn (array $request) => $wiring->callers((string) $request['symbol']),
], $wiring);
