<?php

declare(strict_types=1);

/**
 * Lives in the long-lived driver process and never loads project classes itself.
 * Keeps per-file facts (call sites with resolved receiver types) and per-class facts (hierarchy, methods),
 * each registered against the files it was computed from. On every query the core reports the changed
 * files; their facts and everything depending on them are dropped and recomputed in forked children.
 */
final class FactStore implements BackgroundWork
{
    /** Composer rewrites this on every install/update; vendor code is then reloaded by restarting the driver. */
    private const RESTART_INPUTS = ['vendor/composer/installed.json'];

    /** The name sites of `$this->$method()` are filed under: no method is called that. */
    public const DYNAMIC_NAME = '*';

    private const FILES_PER_WORKER = 40;

    /** Warm-up runs beside queries at the lowest CPU priority, with fewer children than a query uses. */
    private const WARM_UP_WORKERS = 3;

    /**
     * A child keeps every file it parsed, so batches stay small (about 50 MB each); but each child also costs a fork
     * and loads classes anew, which at 10 files took a third of the warm-up's CPU.
     */
    private const WARM_UP_FILES_PER_JOB = 50;

    private const WARM_UP_CLASSES_PER_JOB = 100;

    /**
     * Files questions may read themselves before the warm-up is done. Past it they would read much of the repo
     * twice, beside the warm-up and slower than it: the warm-up is finished first, at full speed.
     */
    private const READ_BEFORE_WARM_UP_LIMIT = 200;

    /** How long a question waits for the warm-up to finish; past it, it reads what it needs itself. */
    private const WARM_UP_WAIT_SECONDS = 120;

    private int $readBeforeWarmUp = 0;

    /** True while a question waits for the warm-up: more children, at normal priority. */
    private bool $warmUpUrgent = false;

    /** A wait for the warm-up ran out of time: later questions read what they need instead of waiting again. */
    private bool $warmUpWaitGaveUp = false;

    /** More files than this changed since the facts were saved (another branch): reading everything is as quick. */
    private const SAVED_CHANGES_LIMIT = 500;

    /** A question no child survived answering: like a changed ancestry, its file is read again whole. */
    private const UNANSWERED = 'ancestry|unanswered';

    /** Bumped whenever file facts change, so they are saved again once the server is idle. */
    private int $factsVersion = 0;

    private int $savedVersion = 0;

    /** Saving forks and writes every file's facts: after edits, at most this often (a later start syncs the rest). */
    private const SAVE_INTERVAL_SECONDS = 60;

    private ?float $lastSave = null;

    /** @var list<string> files still to read in the background */
    private array $warmUpQueue = [];

    /** @var list<string> files to read one per child, after a child reading them together died */
    private array $warmUpSingles = [];

    /** @var array<int, array{0: int, 1: resource, 2: string}> running background children by stream id */
    private array $warmUpJobs = [];

    /** @var array<int, array{0: string, 1: list<string>, 2: int}> [kind, items, change serial at start] by stream id */
    private array $warmUpChunks = [];

    /** @var list<string> classes still to describe in the background, once every file is read */
    private array $classWarmUpQueue = [];

    /** @var array<int, list<string>> paths whose content changed, by update serial */
    private array $changeLog = [];

    private int $changeSerial = 0;

    /** @var array<string, string> files that could not be analysed => why; reported, never silently skipped */
    private array $unreadable = [];

    private ?float $warmUpStarted = null;

    /** True once every PHP file of the repo has facts; a name then needs no search to be complete. */
    private bool $allExtracted = false;

    /**
     * Repo-relative path => what is read of every file a question walks (`hash`, `answerHash`, `deps`, `classes`,
     * `mentions`, `carried`), and the rest packed in Payload strings, a third of the memory of the arrays they hold:
     * `packedSites` (called name => its call sites), `packedScopes`, and `packedRest` (`questions`, `units`, read
     * only when files change). Read through facts(), sitesCalled(), allSites(), scopes() and questions(); written
     * through keep().
     *
     * @var array<string, array>
     */
    private array $fileFacts = [];

    /** The parts of a file's facts kept packed, as facts() returns them. */
    private const PACKED_PARTS = ['byName' => true, 'questions' => true, 'scopes' => true, 'units' => true];

    /** Scopes of the files read last: every caller found in a file names the scope it is in. */
    private const SCOPES_KEPT = 64;

    /** @var array<string, list<array>> path => unpacked scopes, most recently read last */
    private array $unpackedScopes = [];

    /** @var array<string, array<string, true>> lowercase call name => paths having such call sites */
    private array $pathsByName = [];

    /** @var array<string, array<string, true>> lowercase method name => paths declaring a method of that name */
    private array $pathsByMethodName = [];

    /** @var array<string, array<string, true>> lowercase class a call's receiver can be => paths having such calls */
    private array $pathsByReceiverType = [];

    /** @var array<string, string> lowercase receiver class => its name as first written at a call */
    private array $receiverTypeNames = [];

    /** @var array<string, array<string, true>> lowercase class => paths constructing it (`new`, `parent::__construct`) */
    private array $constructions = [];

    /** @var array<string, array{0: string, 1: list<string>}> lowercase declared class => [declared name, lowercase ancestors] */
    private array $declaredClasses = [];

    /** @var array<string, array> lowercase class => class facts */
    private array $classFacts = [];

    /** @var array<string, int> each list of receiver types or question positions sites hold => its index in $lists */
    private array $listIndex = [];

    /** @var list<list<mixed>> one copy of each such list; packed sites hold its index */
    private array $lists = [];

    /** @var array<string, string> lowercase declared class => the file declaring it */
    private array $declaringFile = [];

    /** @var array<string, true> lowercase names that did not resolve to a class; any PHP change may create them */
    private array $missingClasses = [];

    /** @var array<string, true> caller lists a change dropped, recomputed while idle */
    private array $recomputeQueue = [];

    /** @var list<string> files whose content changed in the update in progress */
    private array $changedNow = [];

    /** @var array<string, list<array>> symbol => caller edges, valid until a PHP file or a dependency changes */
    private array $callersCache = [];

    /** @var array<string, array<string, array<string, true>>> 'path'|'name'|'class' => key => cached symbols that used it */
    private array $cacheUsers = ['path' => [], 'name' => [], 'class' => []];

    /** @var array{path: array<string, true>, name: array<string, true>, class: array<string, true>}|null what the callers computation in progress uses */
    private ?array $recording = null;

    /** @var array<string, true> ripgrep searches whose matching files are all extracted */
    private array $scanned = [];

    /** @var array<string, array<string, true>> dependency path => owner keys (`file:<path>` or `class:<name>`) */
    private array $dependents = [];

    /** @var array<string, list<string>> owner key => dependency paths */
    private array $ownerDeps = [];

    public function __construct(
        private readonly PhpFileCache $files,
        private readonly EngineGuard $guard,
        private readonly WorkerPool $pool,
        private readonly int $parallelism,
    ) {}

    /** What this machine's PHP can do for scout, and how far the warm-up got. */
    public function probe(): array
    {
        return [
            'driver' => 'php',
            'php' => PHP_VERSION,
            'fork' => ! ForkedWorker::inline(),
            'opcache' => self::opcacheUsage() !== [],
            'filesRead' => count($this->fileFacts),
            'allFilesRead' => $this->allExtracted,
            'classesDescribed' => count($this->classFacts),
            'unreadable' => $this->unreadable,
        ];
    }

    public function hasIdleWork(): bool
    {
        return $this->pool->needsWorkers() || $this->saveDue() || $this->idleWorkersBehind() || $this->recomputeQueue !== [];
    }

    /** What the driver held when the idle workers were forked: file facts version and classes described. */
    private string $idleWorkersForkedAt = '';

    private function heldFacts(): string
    {
        return $this->factsVersion.'/'.count($this->classFacts);
    }

    /** Facts changed since the idle workers were forked, once the warm-up is over (it changes them all the time). */
    private function idleWorkersBehind(): bool
    {
        return $this->allExtracted && $this->warmUpJobs === [] && $this->idleWorkersForkedAt !== $this->heldFacts();
    }

    /**
     * One small step while nobody is asking: refill the worker pool, else recompute one caller list a change
     * dropped, so the next question about it is answered from memory.
     */
    public function idleStep(): void
    {
        if ($this->pool->needsWorkers()) {
            $this->pool->addOne();
            $this->idleWorkersForkedAt = $this->heldFacts();

            return;
        }
        if ($this->saveDue()) {
            $this->save();

            return;
        }
        if ($this->idleWorkersBehind()) {
            // What the warm-up freed goes back to the system before a worker is forked to share what is left.
            gc_collect_cycles();
            gc_mem_caches();
            $this->pool->renewIdle();
            $this->idleWorkersForkedAt = $this->heldFacts();

            return;
        }
        $symbol = array_key_first($this->recomputeQueue);
        unset($this->recomputeQueue[$symbol]);
        $this->callers($symbol);
    }

    /**
     * Brings the facts up to date with the changed files, recomputing only what can actually differ.
     * Changed files are read again first; their unit fingerprints (function bodies, class shapes) say which
     * units really changed. Facts of other files are kept unless a question they were built on relied on one
     * of those units and now gets a different answer.
     *
     * @param  list<string>  $changed
     */
    public function sync(array $changed): array
    {
        $syncStarted = microtime(true);
        $this->guard->apply($changed, self::RESTART_INPUTS);
        $this->files->changed($changed);
        $root = $this->files->repoRoot;
        $contentChanged = [];
        foreach ($changed as $path) {
            $known = $this->fileFacts[$path]['hash'] ?? null;
            if ($known === null || $known !== FactExtraction::contentHash("{$root}/{$path}")) {
                $contentChanged[] = $path;
            }
        }
        profilePhase('sync: content hashes', $syncStarted, ['changed' => count($changed), 'contentChanged' => count($contentChanged)]);
        if ($contentChanged === []) {
            return ['watch' => self::RESTART_INPUTS, ...$this->warnings()];
        }

        $phaseStarted = microtime(true);
        $this->changedNow = $contentChanged;
        $this->changeSerial++;
        if ($this->warmUpJobs === []) {
            $this->changeLog = [];
        } else {
            $this->changeLog[$this->changeSerial] = $contentChanged;
        }
        // A file PHP aborted on may have depended on one just changed (a broken trait, fixed): it is read again too.
        $this->unreadable = array_filter($this->unreadable, fn (string $path) => is_file("{$root}/{$path}"), ARRAY_FILTER_USE_KEY);
        $reread = array_values(array_unique([...array_filter($contentChanged, fn (string $path) => str_ends_with($path, '.php')), ...array_keys($this->unreadable)]));
        $changedUnits = [];
        $wholeFiles = array_flip(array_diff($contentChanged, $reread));
        $this->reread($reread, $changedUnits, $wholeFiles);
        profilePhase('sync: re-read changed files', $phaseStarted, ['files' => $reread, 'carriedSites' => array_map(fn (string $path) => $this->fileFacts[$path]['carried'] ?? 0, $reread), 'changedUnits' => array_keys($changedUnits)]);

        $suspectFiles = [];
        $suspectClasses = [];
        foreach ([...$contentChanged, ...array_keys($changedUnits)] as $dependency) {
            foreach (array_keys($this->dependents[$dependency] ?? []) as $owner) {
                [$type, $key] = explode(':', $owner, 2);
                if ($type === 'class') {
                    $suspectClasses[$key] = true;
                } elseif (! in_array($key, $reread, true)) {
                    $suspectFiles[$key] = true;
                }
            }
        }
        $unitChanged = fn (string $unit) => isset($changedUnits[$unit]) || (str_contains($unit, '#') && isset($wholeFiles[strstr($unit, '#', true)]));

        $phaseStarted = microtime(true);
        $stale = $this->staleFacts(array_keys($suspectFiles), $unitChanged);
        profilePhase('sync: re-ask questions', $phaseStarted, ['files' => count($suspectFiles), 'stale' => array_keys($stale)]);

        // A changed ancestry reshapes the whole file's facts; any other changed answer only touches the call
        // sites that asked that question.
        $phaseStarted = microtime(true);
        $rereadWhole = [];
        $refresh = [];
        foreach ($stale as $path => $changedQuestions) {
            $ancestryChanged = array_filter(array_keys($changedQuestions), fn (string $question) => str_starts_with($question, 'ancestry|')) !== [];
            if ($ancestryChanged) {
                $rereadWhole[] = $path;
            } else {
                $refresh[$path] = $changedQuestions;
            }
        }
        $this->reread($rereadWhole, $changedUnits, $wholeFiles);
        $unrefreshed = [];
        $refreshedSites = $this->refreshSites($refresh, function (array $job) use (&$unrefreshed) {
            $unrefreshed[] = $job[0];
        });
        $this->reread($unrefreshed, $changedUnits, $wholeFiles);
        profilePhase('sync: refresh stale facts', $phaseStarted, ['wholeFiles' => count($rereadWhole), 'sites' => $refreshedSites]);

        $phaseStarted = microtime(true);
        $refetch = [];
        foreach (array_keys($suspectClasses) as $key) {
            foreach (array_keys($this->classFacts[$key]['units'] ?? ['missing' => true]) as $unit) {
                if ($unitChanged($unit)) {
                    $refetch[$key] = $this->classFacts[$key]['name'] ?? $key;
                    break;
                }
            }
        }
        $previousClasses = [];
        foreach ($refetch as $key => $name) {
            $previousClasses[$key] = $this->classFacts[$key]['answerHash'] ?? null;
            unset($this->classFacts[$key]);
            $this->forgetOwner("class:{$key}");
        }
        $this->ensureClasses(array_values($refetch));
        foreach ($previousClasses as $key => $previous) {
            if ($previous !== ($this->classFacts[$key]['answerHash'] ?? null)) {
                $this->forgetCallersUsing('class', $key);
            }
        }
        profilePhase('sync: refetch classes', $phaseStarted, ['classes' => count($refetch)]);
        profilePhase('sync: total', $syncStarted, ['cachedCallers' => count($this->callersCache), 'opcache' => self::opcacheUsage()]);

        return ['watch' => self::RESTART_INPUTS, ...$this->warnings()];
    }

    /**
     * Reads files again; records which of their units changed (all of them for a file seen for the first time)
     * and drops the cached caller lists their new answers can alter.
     *
     * @param  list<string>  $paths
     * @param  array<string, true>  $changedUnits
     * @param  array<string, true>  $wholeFiles
     */
    private function reread(array $paths, array &$changedUnits, array &$wholeFiles): void
    {
        $previous = [];
        foreach ($paths as $path) {
            $previous[$path] = $this->facts($path);
            $this->forgetFile($path);
        }
        $root = $this->files->repoRoot;
        $this->extractFiles(array_values(array_filter($paths, fn (string $path) => is_file("{$root}/{$path}"))), array_filter($previous), $this->changedNow);

        foreach ($previous as $path => $old) {
            $new = $this->facts($path);
            if ($old === null) {
                $wholeFiles[$path] = true;
            } else {
                foreach (array_keys(($old['units'] ?? []) + ($new['units'] ?? [])) as $key) {
                    if (($old['units'][$key] ?? null) !== ($new['units'][$key] ?? null)) {
                        $changedUnits["{$path}#{$key}"] = true;
                    }
                }
            }
            foreach (array_keys($new['classes'] ?? []) as $declared) {
                $changedUnits["missing-class:{$declared}"] = true;
            }
            if (($old['answerHash'] ?? null) !== ($new['answerHash'] ?? null)) {
                $this->forgetCallersAffectedBy($path, $old, $new);
            }
        }
    }

    /** Drops cached caller lists that a change of one file's facts can alter, and only those. */
    private function forgetCallersAffectedBy(string $path, ?array $old, ?array $new): void
    {
        // Lines inserted or removed around unchanged code move positions only: the kept answers move with them.
        $moved = $old !== null && $new !== null ? self::linesMoved($old, $new) : null;
        if ($moved !== null) {
            $this->moveCachedLines($path, $moved);

            return;
        }
        if (($old['classes'] ?? []) !== ($new['classes'] ?? [])) {
            // A class appeared, vanished or changed ancestry: answers about it, and about its ancestors
            // (whose subclass sets it joins or leaves), may move.
            foreach ([...array_values($old['classes'] ?? []), ...array_values($new['classes'] ?? [])] as [$name, $ancestors]) {
                unset($this->missingClasses[strtolower($name)]);
                foreach ([strtolower($name), ...$ancestors] as $class) {
                    $this->forgetCallersUsing('class', $class);
                }
            }
        }
        // Call sites are compared per name below; the rest of a file's facts (scope names, names it mentions,
        // declarations) is read by every caller list that walked the file, so a change there drops them all.
        if (($old['scopes'] ?? null) !== ($new['scopes'] ?? null) || ($old['mentions'] ?? null) !== ($new['mentions'] ?? null) || ($old['classes'] ?? null) !== ($new['classes'] ?? null)) {
            $this->forgetCallersUsing('path', $path);
        }
        foreach (array_keys(($old['byName'] ?? []) + ($new['byName'] ?? [])) as $name) {
            if (($old['byName'][$name] ?? null) !== ($new['byName'][$name] ?? null)) {
                $this->forgetCallersUsing('name', $name);
            }
        }
    }

    /**
     * When two versions of a file's facts differ only in line numbers (same scopes, same call sites in the same
     * order, same classes and names), old line => new line for every line an answer can cite; else null.
     *
     * @return array<int, int>|null
     */
    private static function linesMoved(array $old, array $new): ?array
    {
        if (array_keys($old['byName']) !== array_keys($new['byName']) || $old['mentions'] !== $new['mentions'] || $old['classes'] !== $new['classes'] || count($old['scopes']) !== count($new['scopes'])) {
            return null;
        }
        $moved = [];
        $pair = function (int $from, int $to) use (&$moved): bool {
            if (isset($moved[$from]) && $moved[$from] !== $to) {
                return false;
            }
            $moved[$from] = $to;

            return true;
        };
        foreach ($old['scopes'] as $index => $scope) {
            $now = $new['scopes'][$index];
            if ([$scope[0], $scope[3], $scope[4]] !== [$now[0], $now[3], $now[4]] || ! $pair($scope[1], $now[1]) || ! $pair($scope[2], $now[2])) {
                return null;
            }
        }
        foreach ($old['byName'] as $name => $sites) {
            if (count($sites) !== count($new['byName'][$name])) {
                return null;
            }
            foreach ($sites as $index => $site) {
                $now = $new['byName'][$name][$index];
                // Name, kind, receiver types and confidence, and the scope it is in: all but where it is.
                if ([$site[0], $site[2], $site[3], $site[4], $site[5]] !== [$now[0], $now[2], $now[3], $now[4], $now[5]] || ! $pair($site[1], $now[1])) {
                    return null;
                }
            }
        }

        return $moved;
    }

    /** Moves the lines of one file in every kept caller list. */
    private function moveCachedLines(string $path, array $moved): void
    {
        $move = fn (?array $place) => $place !== null && ($place['file'] ?? null) === $path && isset($moved[$place['line']]) ? ['line' => $moved[$place['line']]] + $place : $place;
        foreach ($this->callersCache as $symbol => $edges) {
            foreach ($edges as $index => $edge) {
                $edges[$index]['at'] = $move($edge['at'] ?? null);
                $edges[$index]['from'] = $move($edge['from']);
            }
            $this->callersCache[$symbol] = $edges;
        }
    }

    private function forgetCallersUsing(string $kind, string $key): void
    {
        foreach (array_keys($this->cacheUsers[$kind][$key] ?? []) as $symbol) {
            if (isset($this->callersCache[$symbol])) {
                unset($this->callersCache[$symbol]);
                $this->recomputeQueue[$symbol] = true;
            }
        }
        unset($this->cacheUsers[$kind][$key]);
    }

    private function recordUse(string $kind, string $key): void
    {
        if ($this->recording !== null) {
            $this->recording[$kind][$key] = true;
        }
    }

    /**
     * Asks again the questions of these files that relied on a changed unit. Returns, for each file with an
     * answer that differs, the questions that changed. The others keep their facts, with the refreshed units
     * and dependencies.
     *
     * @param  list<string>  $paths
     * @return array<string, array<string, true>>
     */
    private function staleFacts(array $paths, callable $unitChanged): array
    {
        $toAsk = [];
        foreach ($paths as $path) {
            foreach ($this->questions($path) as [$question, , $units]) {
                foreach (array_keys($units) as $unit) {
                    if ($unitChanged($unit)) {
                        $toAsk[$question] = true;
                        break;
                    }
                }
            }
        }
        $answers = [];
        $unanswered = [];
        if ($toAsk !== []) {
            $chunkSize = (int) ceil(count($toAsk) / $this->parallelism);
            $chunks = $this->mapSurviving('answers', array_keys($toAsk), $chunkSize, function (string $question) use (&$unanswered) {
                $unanswered[$question] = true;
            });
            $answers = $chunks === [] ? [] : array_merge(...$chunks);
        }

        $stale = [];
        foreach ($paths as $path) {
            $questions = $this->questions($path);
            $changed = [];
            foreach ($questions as $position => [$question, $hash]) {
                if (isset($unanswered[$question])) {
                    $changed[self::UNANSWERED] = true;
                }
                if (! isset($answers[$question])) {
                    continue;
                }
                if ($answers[$question][0] !== $hash) {
                    $changed[$question] = true;
                }
                $questions[$position] = [$question, ...$answers[$question]];
            }
            if ($changed !== []) {
                $stale[$path] = $changed;
            }
            $this->replaceQuestions($path, $questions);
        }

        return $stale;
    }

    /** Stores a file's refreshed question list and re-registers its dependencies from the units they rely on. */
    private function replaceQuestions(string $path, array $questions): void
    {
        $facts = $this->facts($path);
        if ($facts === null) {
            return;
        }
        $facts['questions'] = $questions;
        $deps = [$path => true];
        foreach ($facts['deps'] as $dep) {
            $deps[$dep] = true;
        }
        foreach ($questions as [, , $units]) {
            foreach (array_keys($units) as $unit) {
                $deps[str_contains($unit, '#') ? strstr($unit, '#', true) : $unit] = true;
            }
        }
        $facts['deps'] = array_keys($deps);
        $this->keep($path, $facts);
        $this->forgetOwner("file:{$path}");
        $this->remember("file:{$path}", $facts['deps']);
    }

    /**
     * Recomputes, in forked children, only the call sites that asked a question whose answer changed, and
     * drops the cached caller lists those sites can alter.
     *
     * @param  array<string, array<string, true>>  $changedQuestions  path => questions whose answer changed
     * @param  callable(array, string): void  $lost  hears of each [path, sites] no child survived refreshing
     */
    private function refreshSites(array $changedQuestions, callable $lost): int
    {
        $requests = [];
        foreach ($changedQuestions as $path => $questions) {
            $positions = [];
            foreach ($this->questions($path) as $position => [$question]) {
                if (isset($questions[$question])) {
                    $positions[$position] = true;
                }
            }
            foreach ($this->allSites($path) as $sites) {
                foreach ($sites as $site) {
                    if (array_intersect_key($positions, array_flip($site[7])) !== []) {
                        $requests[$path][] = $site[6];
                    }
                }
            }
        }
        if ($requests === []) {
            return 0;
        }
        $jobs = array_map(null, array_keys($requests), array_values($requests));
        $results = $this->mapSurviving('refresh', $jobs, (int) ceil(count($jobs) / $this->parallelism), $lost);
        $count = 0;
        foreach ($results === [] ? [] : array_merge(...$results) as $path => $refreshed) {
            $count += count($refreshed);
            $this->applyRefreshedSites($path, $refreshed);
        }

        return $count;
    }

    /** @param array<int, array{0: array|null, 1: list<array>}> $refreshed token index => [site, its questions] */
    private function applyRefreshedSites(string $path, array $refreshed): void
    {
        $facts = $this->facts($path);
        $previousAnswerHash = $facts['answerHash'];
        $questions = $facts['questions'];
        $positionOf = [];
        foreach ($questions as $position => [$question]) {
            $positionOf[$question] = $position;
        }
        $changedNames = [];
        $this->indexConstructions($path, $facts['byName']['__construct'] ?? [], false);
        $this->indexReceivers($path, $facts['byName'], false);
        foreach ($facts['byName'] as $name => $sites) {
            foreach ($sites as $offset => $site) {
                if (! array_key_exists($site[6], $refreshed)) {
                    continue;
                }
                [$newSite, $siteQuestions] = $refreshed[$site[6]];
                $positions = [];
                foreach ($siteQuestions as $record) {
                    $positionOf[$record[0]] ??= count($questions);
                    $questions[$positionOf[$record[0]]] = $record;
                    $positions[] = $positionOf[$record[0]];
                }
                $newSite = $newSite === null ? $site : [...array_slice($newSite, 0, 7), $positions];
                if (array_slice($newSite, 0, 6) !== array_slice($site, 0, 6)) {
                    $changedNames[$name] = true;
                }
                $facts['byName'][$name][$offset] = $newSite;
            }
        }
        $this->keep($path, $facts);
        $this->indexConstructions($path, $facts['byName']['__construct'] ?? [], true);
        $this->indexReceivers($path, $facts['byName'], true);
        $this->replaceQuestions($path, $questions);
        $this->fileFacts[$path]['answerHash'] = self::answerHash($this->facts($path));
        if ($this->fileFacts[$path]['answerHash'] !== $previousAnswerHash) {
            $this->forgetCallersUsing('path', $path);
            foreach (array_keys($changedNames) as $name) {
                $this->forgetCallersUsing('name', $name);
            }
        }
    }

    private function forgetFile(string $path): void
    {
        $facts = $this->facts($path);
        if ($facts === null) {
            return;
        }
        unset($this->unpackedScopes[$path]);
        foreach (array_keys($facts['byName']) as $name) {
            unset($this->pathsByName[$name][$path]);
        }
        $this->indexConstructions($path, $facts['byName']['__construct'] ?? [], false);
        $this->indexReceivers($path, $facts['byName'], false);
        $this->indexMethods($path, $facts['scopes'], false);
        foreach (array_keys($facts['classes']) as $class) {
            unset($this->declaredClasses[$class], $this->declaringFile[$class]);
            $this->descendants = null;
        }
        unset($this->fileFacts[$path]);
        $this->forgetOwner("file:{$path}");
        $this->factsVersion++;
    }

    private function remember(string $owner, array $deps): void
    {
        $this->ownerDeps[$owner] = $deps;
        foreach ($deps as $dep) {
            $this->dependents[$dep][$owner] = true;
        }
    }

    private function forgetOwner(string $owner): void
    {
        foreach ($this->ownerDeps[$owner] ?? [] as $dep) {
            unset($this->dependents[$dep][$owner]);
        }
        unset($this->ownerDeps[$owner]);
    }

    /**
     * @param  list<string>  $paths
     * @param  array<string, array>  $previous  facts the files had before their latest change, to carry over what still holds
     * @param  list<string>  $changedPaths  every file changed in the same update
     */
    private function extractFiles(array $paths, array $previous = [], array $changedPaths = []): void
    {
        $missing = array_values(array_unique(array_filter($paths, fn (string $path) => ! isset($this->fileFacts[$path]))));
        if ($missing === []) {
            return;
        }
        if ($this->warmUpQueue !== []) {
            // Read here, so the warm-up need not read them again.
            $this->warmUpQueue = array_values(array_diff($this->warmUpQueue, $missing));
        }
        $missing = array_map(fn (string $path) => [$path, $previous[$path] ?? null, $changedPaths], $missing);
        $chunkSize = min(self::FILES_PER_WORKER, (int) ceil(count($missing) / $this->parallelism));
        $chunks = $this->mapSurviving('files', $missing, $chunkSize, function (array $item, string $error) {
            $this->unreadable[$item[0]] = $error;
        });
        foreach ($chunks as $facts) {
            foreach ($facts as $path => $fileFacts) {
                if ($fileFacts !== null) {
                    $this->storeFile($path, $fileFacts);
                }
            }
        }
    }

    /**
     * The pool's results for $items, a chunk per child, as WorkerPool::map() returns them. PHP aborts on some broken
     * classes: when a child dies, each item is asked again in a child of its own, and $lost hears of every item no
     * child survived, so a broken file costs only what depends on it and never the driver.
     *
     * @param  callable(mixed, string): void  $lost
     * @return list<mixed>
     */
    private function mapSurviving(string $kind, array $items, int $chunkSize, callable $lost): array
    {
        try {
            return $this->pool->map($kind, $items, $chunkSize);
        } catch (RuntimeException) {
            $chunks = [];
            foreach ($items as $item) {
                try {
                    $chunks[] = $this->pool->map($kind, [$item], 1)[0];
                } catch (RuntimeException $error) {
                    $lost($item, $error->getMessage());
                }
            }

            return $chunks;
        }
    }

    /** A file's facts whole, in the form storeFile() builds; null when the file is not read. */
    private function facts(string $path): ?array
    {
        $kept = $this->fileFacts[$path] ?? null;
        if ($kept === null) {
            return null;
        }
        [$questions, $units] = Payload::decode($kept['packedRest'], false);

        return array_diff_key($kept, ['packedSites' => true, 'packedScopes' => true, 'packedRest' => true])
            + ['byName' => $this->allSites($path), 'questions' => $questions, 'scopes' => $this->scopes($path), 'units' => $units];
    }

    /** @return list<array> a file's call sites of one called name */
    private function sitesCalled(string $path, string $name): array
    {
        $packed = $this->fileFacts[$path]['packedSites'][$name] ?? null;

        return $packed === null ? [] : $this->unpackSites($packed);
    }

    /** @return array<string, list<array>> a file's call sites by called name */
    private function allSites(string $path): array
    {
        return array_map(fn (string $packed) => $this->unpackSites($packed), $this->fileFacts[$path]['packedSites'] ?? []);
    }

    /** @return list<array> */
    private function scopes(string $path): array
    {
        if (isset($this->unpackedScopes[$path])) {
            $scopes = $this->unpackedScopes[$path];
            unset($this->unpackedScopes[$path]);

            return $this->unpackedScopes[$path] = $scopes;
        }
        if (! isset($this->fileFacts[$path])) {
            return [];
        }
        if (count($this->unpackedScopes) >= self::SCOPES_KEPT) {
            unset($this->unpackedScopes[array_key_first($this->unpackedScopes)]);
        }

        return $this->unpackedScopes[$path] = Payload::decode($this->fileFacts[$path]['packedScopes'], false);
    }

    /** @return list<array> the questions a file's facts were built on */
    private function questions(string $path): array
    {
        return isset($this->fileFacts[$path]) ? Payload::decode($this->fileFacts[$path]['packedRest'], false)[0] : [];
    }

    /** @return list<array> */
    private function unpackSites(string $packed): array
    {
        $sites = Payload::decode($packed, false);
        foreach ($sites as &$site) {
            if (is_int($site[3])) {
                $site[3] = $this->lists[$site[3]];
            }
            if (is_int($site[7] ?? null)) {
                $site[7] = $this->lists[$site[7]];
            }
        }

        return $sites;
    }

    /** Keeps a file's facts whole as facts() returns them, packing what only questions about the file read. */
    private function keep(string $path, array $facts): void
    {
        $packedSites = [];
        foreach ($facts['byName'] as $name => $sites) {
            // Receiver types and question positions repeat across most sites (15,000 distinct type lists among
            // 290,000 sites on a large backend): a packed site holds the index of the one kept copy of its list.
            foreach ($sites as &$site) {
                if ($site[3] !== null) {
                    $site[3] = $this->listIndex[implode("\0", $site[3])] ??= array_push($this->lists, $site[3]) - 1;
                }
                if (isset($site[7])) {
                    $site[7] = $this->listIndex[implode("\0", $site[7])] ??= array_push($this->lists, $site[7]) - 1;
                }
            }
            unset($site);
            $packedSites[$name] = Payload::encode($sites);
        }
        unset($this->unpackedScopes[$path]);
        $this->fileFacts[$path] = array_diff_key($facts, self::PACKED_PARTS) + [
            'packedSites' => $packedSites,
            'packedScopes' => Payload::encode($facts['scopes']),
            'packedRest' => Payload::encode([$facts['questions'] ?? [], $facts['units'] ?? []]),
        ];
    }

    private function storeFile(string $path, array $fileFacts): void
    {
        unset($this->unreadable[$path]);
        $byName = [];
        foreach ($fileFacts['sites'] as $site) {
            $byName[$site[0]][] = $site;
        }
        unset($fileFacts['sites']);
        $fileFacts['byName'] = $byName;
        $fileFacts['mentions'] = array_flip($fileFacts['mentions']);
        $fileFacts['answerHash'] ??= self::answerHash($fileFacts);
        $this->keep($path, $fileFacts);
        $this->index($path, $fileFacts);
    }

    /** Files a kept file's facts under the names, classes and dependencies questions look it up by. */
    private function index(string $path, array $fileFacts): void
    {
        foreach (array_keys($fileFacts['byName']) as $name) {
            $this->pathsByName[$name][$path] = true;
        }
        $this->indexConstructions($path, $fileFacts['byName']['__construct'] ?? [], true);
        $this->indexReceivers($path, $fileFacts['byName'], true);
        $this->indexMethods($path, $fileFacts['scopes'], true);
        // Key by key: `$new + $all` would copy every class known so far for each file read.
        foreach ($fileFacts['classes'] as $class => $declared) {
            $this->declaredClasses[$class] = $declared;
            $this->declaringFile[$class] = $path;
        }
        if ($fileFacts['classes'] !== []) {
            $this->descendants = null;
        }
        $this->remember("file:{$path}", $fileFacts['deps']);
        $this->factsVersion++;
    }

    /** @param list<array> $sites `__construct` sites */
    private function indexConstructions(string $path, array $sites, bool $add): void
    {
        foreach ($sites as $site) {
            foreach ($site[3] ?? [] as $type) {
                if ($add) {
                    $this->constructions[strtolower(ltrim($type, '\\'))][$path] = true;
                } else {
                    unset($this->constructions[strtolower(ltrim($type, '\\'))][$path]);
                }
            }
        }
    }

    /** @param array<string, list<array>> $byName a file's call sites by called name */
    private function indexReceivers(string $path, array $byName, bool $add): void
    {
        foreach ($byName as $sites) {
            foreach ($sites as $site) {
                foreach ($site[2] === 'function' ? [] : ($site[3] ?? []) as $type) {
                    $name = ltrim($type, '\\');
                    $key = strtolower($name);
                    if ($add) {
                        $this->pathsByReceiverType[$key][$path] = true;
                        $this->receiverTypeNames[$key] ??= $name;
                    } else {
                        unset($this->pathsByReceiverType[$key][$path]);
                    }
                }
            }
        }
    }

    private function indexMethods(string $path, array $scopes, bool $add): void
    {
        foreach ($scopes as $scope) {
            $method = self::declaredMethodName($scope);
            if ($method === null) {
                continue;
            }
            if ($add) {
                $this->pathsByMethodName[$method][$path] = true;
            } else {
                unset($this->pathsByMethodName[$method][$path]);
            }
        }
    }

    /** The lowercase name of the method a scope declares; null for a function, closure or test scope. */
    private static function declaredMethodName(array $scope): ?string
    {
        return ! $scope[3] && str_contains($scope[0], '::') ? strtolower(substr(strrchr($scope[0], ':'), 1)) : null;
    }

    /** Fingerprint of everything a file's facts answer with (positions included, question bookkeeping excluded). */
    private static function answerHash(array $fileFacts): string
    {
        $sites = [];
        foreach ($fileFacts['byName'] as $name => $byName) {
            foreach ($byName as $site) {
                $sites[$name][] = array_slice($site, 0, 6);
            }
        }

        return hash('xxh128', serialize([$fileFacts['scopes'], $sites, $fileFacts['mentions'], $fileFacts['classes']]));
    }

    /** @param list<string> $classes */
    private function ensureClasses(array $classes): void
    {
        $missing = [];
        foreach ($classes as $class) {
            $class = ltrim($class, '\\');
            $this->recordUse('class', strtolower($class));
            if ($class !== '' && ! isset($this->classFacts[strtolower($class)]) && ! isset($this->missingClasses[strtolower($class)])) {
                $missing[strtolower($class)] = $class;
            }
        }
        if ($missing === []) {
            return;
        }
        $started = microtime(true);
        // A class no child survived describing is unknown, like one that does not exist.
        $lost = [];
        $items = array_map(fn (string $class) => [$class, $this->declaredAlongside($class)], array_values($missing));
        $chunks = $this->mapSurviving('classes', $items, (int) ceil(count($missing) / $this->parallelism), function (array $item) use (&$lost) {
            $lost[strtolower($item[0])] = true;
        });
        profilePhase('classes: fetched', $started, ['count' => count($missing)]);
        $requested = array_keys(array_diff_key($missing, $lost));
        foreach ($chunks === [] ? [] : array_merge(...$chunks) as $index => $facts) {
            $this->storeClass($requested[$index], $facts);
        }
        foreach (array_keys($lost) as $class) {
            $this->storeClass($class, null);
        }
    }

    /**
     * The other classes declared in the file declaring $class: loading one makes $class known when Composer cannot
     * find it by its name (a stub class written in a test file), whichever classes a child happened to load before.
     *
     * @return list<string>
     */
    private function declaredAlongside(string $class): array
    {
        $path = $this->declaringFile[strtolower(ltrim($class, '\\'))] ?? null;
        $declared = $path === null ? [] : array_column($this->fileFacts[$path]['classes'] ?? [], 0);

        return array_values(array_filter($declared, fn (string $name) => strcasecmp($name, ltrim($class, '\\')) !== 0));
    }

    private function storeClass(string $requested, ?array $facts): void
    {
        if ($facts === null) {
            $this->missingClasses[$requested] = true;

            return;
        }
        $key = strtolower($facts['name']);
        // Saved facts carry their hash. Where the class and its methods start is not part of any caller list: moving
        // them changes no answer.
        if (! isset($facts['answerHash'])) {
            $placeless = array_diff_key($facts, ['deps' => true, 'line' => true]);
            $placeless['methods'] = array_map(fn (array $method) => array_diff_key($method, [3 => true]), $facts['methods']);
            $facts['answerHash'] = FactExtraction::answerHash($placeless);
        }
        $facts['ancestorKeys'] = array_flip(array_map('strtolower', $facts['ancestors']));
        $this->classFacts[$key] = $facts;
        $this->remember("class:{$key}", $facts['deps']);
    }

    private function classFactsFor(string $class): ?array
    {
        $this->recordUse('class', strtolower(ltrim($class, '\\')));
        $this->ensureClasses([$class]);

        return $this->classFacts[strtolower(ltrim($class, '\\'))] ?? null;
    }

    /**
     * Reads every PHP file of the repo in the background, so the first question about any name is answered
     * from memory. Results are kept only if the file still has the content they were read from.
     */
    public function startWarmUp(): void
    {
        $this->warmUpStarted = microtime(true);
        $phpFiles = $this->files->phpFiles();
        $this->restoreSaved($phpFiles);
        $this->warmUpQueue = array_values(array_diff($phpFiles, array_keys($this->fileFacts)));
        $this->fillWarmUp();
    }

    /** Where this repository's file facts are saved between servers (beside the other drivers' saved facts). */
    private function savedPath(?string $repoRoot = null): string
    {
        $cache = getenv('SCOUT_CACHE') ?: (getenv('HOME') ?: sys_get_temp_dir()).'/.cache/scout';

        return $cache.'/'.substr(sha1($repoRoot ?? $this->files->repoRoot), 0, 16).'/php-facts';
    }

    /** Saved facts hold only for the same driver code, PHP and Composer setup (autoloading, test paths). */
    private function savedKey(): string
    {
        return $this->inputsKey(['composer.json', ...self::RESTART_INPUTS, ...$this->relativeGlob('vendor/composer/autoload_*.php')]);
    }

    /**
     * What facts saved by another checkout of the same repository (a git worktree) must share to be a starting point
     * here: driver code, PHP and the installed packages. Composer's generated autoload files are left out: they list
     * the application's own classes, which differ between branches, and every file that differs goes through
     * sync() anyway.
     */
    private function seedKey(): string
    {
        return $this->inputsKey(['composer.json', 'composer.lock', ...self::RESTART_INPUTS]);
    }

    /** @param list<string> $repoFiles paths relative to the repository root */
    private function inputsKey(array $repoFiles): string
    {
        $root = $this->files->repoRoot;
        $inputs = [PHP_VERSION];
        foreach (glob(__DIR__.'/*.php') as $driverFile) {
            $inputs[] = basename($driverFile).'='.FactExtraction::contentHash($driverFile);
        }
        foreach ($repoFiles as $path) {
            $inputs[] = $path.'='.FactExtraction::contentHash("{$root}/{$path}");
        }

        return hash('xxh128', implode("\n", $inputs));
    }

    /** @return list<string> */
    private function relativeGlob(string $pattern): array
    {
        $root = $this->files->repoRoot;

        return array_map(fn (string $path) => substr($path, strlen($root) + 1), glob("{$root}/{$pattern}") ?: []);
    }

    /**
     * Facts another checkout of this repository saved, when this one has none of its own: the newest that was saved
     * for this driver, PHP and installed packages, among the checkouts the core names (`SCOUT_SEED_REPOS`, one path
     * per line). An older checkout's facts may still match when a newer one's were saved by another driver version.
     */
    private function seedFacts(): array|false
    {
        $candidates = [];
        foreach (array_filter(explode("\n", (string) getenv('SCOUT_SEED_REPOS'))) as $sibling) {
            $path = $this->savedPath($sibling);
            $modified = @filemtime($path);
            if ($modified !== false) {
                $candidates[$path] = $modified;
            }
        }
        arsort($candidates);
        $wanted = $this->seedKey();
        foreach (array_keys($candidates) as $path) {
            $data = @file_get_contents($path);
            $saved = $data === false ? false : Payload::decode((string) @gzdecode($data), false);
            if (is_array($saved) && ($saved['seedKey'] ?? null) === $wanted) {
                return $saved;
            }
        }

        return false;
    }

    /**
     * Reads back the facts a previous server saved. Files changed, added or removed since then go through sync(),
     * as edits made while a server runs do, so facts built on them are refreshed; the rest is not read again.
     *
     * @param  list<string>  $phpFiles
     */
    private function restoreSaved(array $phpFiles): void
    {
        $started = microtime(true);
        $data = @file_get_contents($this->savedPath());
        $seeded = $data === false;
        $saved = $seeded ? $this->seedFacts() : Payload::decode((string) @gzdecode($data), false);
        $unused = fn (string $reason) => profilePhase('start: saved facts not used', $started, ['reason' => $reason, 'seeded' => $seeded]);
        if (! is_array($saved)) {
            return;
        }
        if (! $seeded && ($saved['key'] ?? null) !== $this->savedKey()) {
            $unused('saved for another driver, PHP or Composer setup');

            return;
        }
        $root = $this->files->repoRoot;
        // A saved file no longer among the repo's PHP files (ignored since, for example) would keep facts a fresh
        // start does not read: read all again. One that another checkout has and this one lacks on disk is a
        // deletion, which sync() handles below like any other.
        foreach (array_keys(array_diff_key($saved['files'], array_flip($phpFiles))) as $path) {
            if (! $seeded || file_exists("{$root}/{$path}")) {
                $unused("{$path} is no longer among the PHP files");

                return;
            }
        }
        // Files outside the repo's PHP files that facts were built on (vendor code): any change there, read all again.
        foreach ($saved['outside'] as $path => $hash) {
            if (FactExtraction::contentHash("{$root}/{$path}") !== $hash) {
                $unused("{$path} changed");

                return;
            }
        }
        // New files (git also lists a tracked file deleted from the working tree: nothing to read there).
        $changed = array_values(array_filter(array_diff($phpFiles, array_keys($saved['files'])), fn (string $path) => is_file("{$root}/{$path}")));
        foreach ($saved['files'] as $path => $facts) {
            if (FactExtraction::contentHash("{$root}/{$path}") !== $facts['hash']) {
                $changed[] = $path;
            }
        }
        if (count($changed) > self::SAVED_CHANGES_LIMIT) {
            $unused(count($changed).' files changed');

            return;
        }
        // Saved as kept, packed parts and the lists their sites point into included: nothing is packed again. The
        // store is still empty here (the warm-up starts before any question), so the saved list indexes hold.
        $this->lists = $saved['lists'];
        foreach ($this->lists as $index => $list) {
            $this->listIndex[implode("\0", $list)] = $index;
        }
        foreach ($saved['files'] as $path => $kept) {
            $this->fileFacts[$path] = $kept;
            $this->index($path, ['byName' => $this->allSites($path), 'scopes' => $this->scopes($path)] + $kept);
        }
        $savedFiles = count($saved['files']);
        // Classes described before a restart, unless a file they were read from changed: describing them all again
        // is most of the CPU a start from saved facts would cost.
        $changedFiles = array_flip($changed);
        foreach ($saved['classes'] ?? [] as $key => $classFacts) {
            $stale = array_filter($classFacts['deps'], fn (string $dependency) => isset($changedFiles[$dependency]) || str_starts_with($dependency, 'missing-class:')) !== [];
            if (! $stale) {
                $this->storeClass($key, $classFacts);
            }
        }
        profilePhase($seeded ? 'start: facts of another checkout read' : 'start: saved facts read', $started, ['files' => $savedFiles, 'changed' => count($changed)]);
        // What sync() changes is saved again once the server is idle; facts taken from another checkout are saved
        // as this one's own.
        $this->savedVersion = $seeded ? -1 : $this->factsVersion;
        if ($changed !== []) {
            $this->sync($changed);
        }
    }

    private function saveDue(): bool
    {
        return $this->allExtracted && $this->warmUpJobs === [] && $this->factsVersion !== $this->savedVersion
            && ($this->lastSave === null || microtime(true) - $this->lastSave > self::SAVE_INTERVAL_SECONDS);
    }

    /**
     * Writes every file's facts in a forked child, so a question arriving meanwhile is not kept waiting; the child
     * writes them from its copy of the memory, which costs the driver nothing.
     */
    private function save(): void
    {
        $root = $this->files->repoRoot;
        $stored = $this->fileFacts;
        $classes = $this->classFacts;
        $path = $this->savedPath();
        $key = $this->savedKey();
        $seedKey = $this->seedKey();
        $job = ForkedWorker::startBackground(function () use ($root, $stored, $classes, $path, $key, $seedKey) {
            $started = microtime(true);
            $outside = [];
            foreach ($stored as $facts) {
                foreach ($facts['deps'] as $dependency) {
                    if (! isset($stored[$dependency]) && str_ends_with($dependency, '.php')) {
                        $outside[$dependency] ??= FactExtraction::contentHash("{$root}/{$dependency}");
                    }
                }
            }
            // Files described classes were read from that are not among the repository's PHP files (an ignored
            // folder Composer still loads): checked at the next start like those of file facts.
            foreach ($classes as $facts) {
                foreach ($facts['deps'] as $dependency) {
                    if (! isset($stored[$dependency]) && str_ends_with($dependency, '.php')) {
                        $outside[$dependency] ??= FactExtraction::contentHash("{$root}/{$dependency}");
                    }
                }
            }
            @mkdir(dirname($path), 0777, true);
            // A full disk only costs the next server a slower start.
            // Without what storeClass() derives again cheaply.
            $classes = array_map(fn (array $facts) => array_diff_key($facts, ['ancestorKeys' => true]), $classes);
            // Files as kept, packed parts included, with the lists packed sites point into.
            $saved = ['key' => $key, 'seedKey' => $seedKey, 'files' => $stored, 'lists' => $this->lists, 'classes' => $classes, 'outside' => $outside];
            if (@file_put_contents("{$path}.tmp", gzencode(Payload::encode($saved), 1)) !== false) {
                @rename("{$path}.tmp", $path);
            }
            profilePhase('saved facts written', $started, ['files' => count($stored), 'bytes' => @filesize($path)]);

            return true;
        }, 'save facts');
        $this->warmUpJobs[(int) $job[1]] = $job;
        $this->warmUpChunks[(int) $job[1]] = ['save', [], $this->changeSerial];
        $this->savedVersion = $this->factsVersion;
        $this->lastSave = microtime(true);
    }

    public function streams(): array
    {
        return array_map(fn (array $job) => $job[1], array_values($this->warmUpJobs));
    }

    public function onReadable($stream): void
    {
        $key = (int) $stream;
        [$finished, $facts] = ForkedWorker::readAvailable($this->warmUpJobs[$key]);
        if (! $finished) {
            return;
        }
        [$kind, $chunk, $startSerial] = $this->warmUpChunks[$key];
        unset($this->warmUpJobs[$key], $this->warmUpChunks[$key]);
        if ($kind === 'save') {
            return;
        }
        if ($kind === 'classes') {
            $this->storeWarmClasses($chunk, $facts, $startSerial);
            $this->fillWarmUp();

            return;
        }
        if ($facts === null) {
            $this->childDied($chunk);
            $this->fillWarmUp();

            return;
        }
        $root = $this->files->repoRoot;
        foreach ($facts as $path => $fileFacts) {
            if ($fileFacts === null) {
                $this->unreadable[$path] = 'file not readable';
            } elseif (! isset($this->fileFacts[$path]) && $fileFacts['hash'] === FactExtraction::contentHash("{$root}/{$path}")) {
                $this->storeFile($path, $fileFacts);
            }
        }
        $this->fillWarmUp();
    }

    /** Reads the rest of the repo's files now, with every core; false if that took too long (it goes on behind). */
    private function finishWarmUp(): bool
    {
        $started = microtime(true);
        $this->warmUpUrgent = true;
        $this->fillWarmUp();
        while (! $this->allExtracted && microtime(true) - $started < self::WARM_UP_WAIT_SECONDS) {
            $read = $this->streams();
            if ($read === []) {
                break;
            }
            $write = $except = null;
            if (@stream_select($read, $write, $except, 1) > 0) {
                foreach ($read as $stream) {
                    $this->onReadable($stream);
                }
            }
        }
        $this->warmUpUrgent = false;
        $this->warmUpWaitGaveUp = ! $this->allExtracted;
        profilePhase('warm-up: finished for a question', $started, ['done' => $this->allExtracted]);

        return $this->allExtracted;
    }

    private function fillWarmUp(): void
    {
        $root = $this->files->repoRoot;
        $workers = $this->warmUpUrgent ? max(self::WARM_UP_WORKERS, 2 * $this->parallelism) : self::WARM_UP_WORKERS;
        while (count($this->warmUpJobs) < $workers && ($this->warmUpQueue !== [] || $this->warmUpSingles !== [])) {
            $chunk = $this->warmUpSingles !== [] ? [array_shift($this->warmUpSingles)] : array_splice($this->warmUpQueue, 0, self::WARM_UP_FILES_PER_JOB);
            // While a question waits, one worker always runs without a machine-wide slot: the question moves on even
            // when other repositories hold every slot (and the core, seeing work done, does not take it for a hang).
            $free = $this->warmUpUrgent && ! in_array(true, array_column($this->warmUpChunks, 3), true);
            $job = ForkedWorker::startBackground(function () use ($root, $chunk) {
                // The warm-up compiles most of the repo once; keep OPcache's shared memory for query workers.
                ini_set('opcache.enable', '0');
                $extraction = new FactExtraction(new PhpFileCache($root));
                $facts = [];
                foreach ($chunk as $path) {
                    $facts[$path] = $extraction->fileFacts($path);
                }

                return $facts;
            }, 'warm-up files', ! $this->warmUpUrgent, ! $free);
            $this->warmUpJobs[(int) $job[1]] = $job;
            $this->warmUpChunks[(int) $job[1]] = ['files', $chunk, $this->changeSerial, $free];
        }
        if ($this->allExtracted) {
            $this->fillClassWarmUp();

            return;
        }
        if ($this->warmUpJobs !== [] || $this->warmUpStarted === null) {
            return;
        }
        // Files that changed while being read were not kept: read them again until none is missing.
        $missing = array_values(array_diff($this->files->phpFiles(), array_keys($this->fileFacts), array_keys($this->unreadable)));
        if ($missing !== []) {
            $this->warmUpQueue = $missing;
            $this->fillWarmUp();

            return;
        }
        $this->allExtracted = true;
        // Every name is now answered from the facts: searches, and so the word index, are not needed any more.
        $this->files->forgetWordIndex();
        $usage = getrusage(1);
        profilePhase('warm-up: done', $this->warmUpStarted, [
            'files' => count($this->fileFacts),
            'childrenCpuSeconds' => round($usage['ru_utime.tv_sec'] + $usage['ru_utime.tv_usec'] / 1e6 + $usage['ru_stime.tv_sec'] + $usage['ru_stime.tv_usec'] / 1e6, 1),
            'driverResidentMb' => (int) (preg_match('/VmRSS:\s+(\d+)/', (string) file_get_contents('/proc/self/status'), $match) ? $match[1] / 1024 : 0),
            'opcache' => self::opcacheUsage(),
        ]);
        $this->warmUpStarted = microtime(true);
        $this->classWarmUpQueue = $this->classesWorthDescribing();
        $this->fillClassWarmUp();
    }

    /** Every declared class, and every class a call site's receiver can be. */
    private function classesWorthDescribing(): array
    {
        $names = [];
        foreach ($this->declaredClasses as [$name]) {
            $names[strtolower($name)] = $name;
        }
        foreach ($this->pathsByReceiverType as $key => $paths) {
            if ($paths !== []) {
                $names[$key] ??= $this->receiverTypeNames[$key];
            }
        }

        return array_values(array_diff_key($names, $this->classFacts, $this->missingClasses));
    }

    private function fillClassWarmUp(): void
    {
        $root = $this->files->repoRoot;
        while (count($this->warmUpJobs) < self::WARM_UP_WORKERS && $this->classWarmUpQueue !== []) {
            $chunk = array_splice($this->classWarmUpQueue, 0, self::WARM_UP_CLASSES_PER_JOB);
            $items = array_map(fn (string $class) => [$class, $this->declaredAlongside($class)], $chunk);
            $job = ForkedWorker::startBackground(function () use ($root, $items) {
                ini_set('opcache.enable', '0');
                $extraction = new FactExtraction(new PhpFileCache($root));

                return array_map(fn (array $item) => $extraction->classFacts(...$item), $items);
            }, 'warm-up classes', takesSlot: true);
            $this->warmUpJobs[(int) $job[1]] = $job;
            $this->warmUpChunks[(int) $job[1]] = ['classes', $chunk, $this->changeSerial];
        }
        if ($this->warmUpJobs === [] && $this->warmUpStarted !== null) {
            profilePhase('warm-up: classes done', $this->warmUpStarted, [
                'classes' => count($this->classFacts),
                'missing' => count($this->missingClasses),
                'driverResidentMb' => (int) (preg_match('/VmRSS:\s+(\d+)/', (string) file_get_contents('/proc/self/status'), $match) ? $match[1] / 1024 : 0),
            ]);
            $this->warmUpStarted = null;
        }
    }

    /**
     * Keeps background class facts unless a file they rely on changed after the job started (those classes are
     * described again when a query needs them).
     */
    private function storeWarmClasses(array $classes, ?array $facts, int $startSerial): void
    {
        if (! is_array($facts)) {
            return;
        }
        $changedSince = [];
        foreach ($this->changeLog as $serial => $paths) {
            if ($serial > $startSerial) {
                $changedSince += array_flip($paths);
            }
        }
        foreach ($classes as $index => $class) {
            $key = strtolower($class);
            $classFacts = $facts[$index] ?? null;
            if (isset($this->classFacts[$key]) || ($changedSince !== [] && ($classFacts === null || array_intersect_key(array_flip($classFacts['deps']), $changedSince) !== []))) {
                continue;
            }
            $this->storeClass($key, $classFacts);
        }
    }

    /** @return array<string, mixed> how full OPcache's shared memory is (it stops caching when full) */
    private static function opcacheUsage(): array
    {
        $status = function_exists('opcache_get_status') ? opcache_get_status(false) : false;
        if (! is_array($status)) {
            return [];
        }

        return [
            'usedMb' => (int) ($status['memory_usage']['used_memory'] / 1048576),
            'freeMb' => (int) ($status['memory_usage']['free_memory'] / 1048576),
            'scripts' => $status['opcache_statistics']['num_cached_scripts'],
            'full' => $status['cache_full'],
            'stringsFull' => $status['interned_strings_usage']['free_memory'] === 0,
        ];
    }

    /** A child died reading these files (PHP can abort on a broken class): retry one by one to find which. */
    private function childDied(array $chunk): void
    {
        if (count($chunk) === 1) {
            $this->unreadable[$chunk[0]] = 'PHP aborted while analysing it';
        } else {
            array_push($this->warmUpSingles, ...$chunk);
        }
    }

    /** @return list<string> */
    /** @return array{warnings: list<string>, testWarnings: list<string>} what a test file hides matters only with tests shown */
    public function warnings(): array
    {
        $lists = ['warnings' => [], 'testWarnings' => []];
        foreach ($this->unreadable as $path => $reason) {
            $lists[$this->files->isTestPath($path) ? 'testWarnings' : 'warnings'][] = "{$path} could not be analysed ({$reason}); callers inside it are unknown";
        }
        if (ForkedWorker::inline()) {
            $lists['warnings'][] = 'PHP has no pcntl here: the driver works in one process and restarts after an edit to a file it loaded (slower, still current)';
        }

        return $lists;
    }

    /** Makes sure every PHP file matching the search has been extracted; later changes keep it true via sync. */
    private function ensureScanned(string $pattern, bool $regex): void
    {
        if ($this->allExtracted) {
            return;
        }
        $key = ($regex ? 're:' : 'word:').strtolower($pattern);
        if (isset($this->scanned[$key])) {
            return;
        }
        $arguments = $regex ? ['--ignore-case', '-e', $pattern] : ['--ignore-case', '--word-regexp', '--fixed-strings', '-e', $pattern];
        $paths = $this->files->ripgrep($arguments);
        $this->readBeforeWarmUp += count(array_diff_key(array_flip($paths), $this->fileFacts));
        if ($this->readBeforeWarmUp > self::READ_BEFORE_WARM_UP_LIMIT && ! $this->warmUpWaitGaveUp && $this->finishWarmUp()) {
            return;
        }
        $this->extractFiles($paths);
        $this->scanned[$key] = true;
    }

    private static function shortName(string $class): string
    {
        return substr(strrchr('\\'.$class, '\\'), 1);
    }

    /** @var array<string, array<string, string>>|null lowercase class => declared descendants (lowercase => name) */
    private ?array $descendants = null;

    /** Every class extending, implementing or using (a trait) $class, however deep. */
    private function subclasses(string $class): array
    {
        // Once every file is read, each class's ancestors (all of them, not only its parent) answer it at once.
        if ($this->allExtracted) {
            if ($this->descendants === null) {
                $this->descendants = [];
                foreach ($this->declaredClasses as $declared => [$name, $ancestors]) {
                    foreach ($ancestors as $ancestor) {
                        $this->descendants[$ancestor][$declared] = $name;
                    }
                }
            }

            return array_values($this->descendants[strtolower($class)] ?? []);
        }
        $found = [];
        $queue = [strtolower($class)];
        while ($queue !== []) {
            $parent = array_shift($queue);
            $short = preg_quote(self::shortName($parent));
            $this->ensureScanned("(extends|implements|use)\\b[^{;]*\\b{$short}\\b", true);
            foreach ($this->declaredClasses as $declared => [$name, $ancestors]) {
                if (! isset($found[$declared]) && in_array($parent, $ancestors, true)) {
                    $found[$declared] = $name;
                    $queue[] = $declared;
                }
            }
        }

        return array_values($found);
    }

    public function callers(string $symbol): array
    {
        if (! isset($this->callersCache[$symbol])) {
            $started = microtime(true);
            $this->recording = ['path' => [], 'name' => [], 'class' => []];
            $this->callersCache[$symbol] = $this->computeCallers($symbol);
            foreach ($this->recording as $kind => $keys) {
                foreach (array_keys($keys) as $key) {
                    $this->cacheUsers[$kind][$key][$symbol] = true;
                }
            }
            $this->recording = null;
            profilePhase('callers: computed', $started, ['symbol' => $symbol]);
        }

        return $this->callersCache[$symbol];
    }

    private function computeCallers(string $symbol): array
    {
        // Another language's symbol (`path#name`): no PHP name contains these.
        if (str_contains($symbol, '#') || str_contains($symbol, '/')) {
            return [];
        }
        if (str_contains($symbol, '::')) {
            [$class, $method] = explode('::', $symbol, 2);
        } elseif ($this->classFactsFor($symbol) !== null) {
            [$class, $method] = [$symbol, '__construct'];
        } else {
            return $this->functionCallers($symbol);
        }
        $target = $this->classFactsFor($class);
        if ($target === null) {
            return $this->anonymousClassCallers($class, $method);
        }
        $name = strtolower($method);
        if ($name === '__call' || $name === '__callstatic') {
            return $this->magicCallers($target, $name === '__callstatic');
        }
        $isConstructor = $name === '__construct';
        $targetDeclared = $this->methodOf($target, $name);
        $targetDeclaring = $targetDeclared === null ? strtolower($target['name']) : self::methodSource($targetDeclared);
        // An abstract or interface method has no body of its own: a call to any implementation goes through it.
        $targetAbstract = ! empty($targetDeclared[5]);

        $family = [$target['name'], ...$this->subclasses($target['name'])];
        $searchNames = $family;
        if (! $isConstructor) {
            $this->ensureClasses($target['ancestors']);
            foreach ($target['ancestors'] as $ancestor) {
                $ancestorFacts = $this->classFacts[strtolower($ancestor)] ?? null;
                // A trait's name is written where a class uses it, not where its objects are: no hint for receivers.
                if ($ancestorFacts !== null && empty($ancestorFacts['trait']) && $this->methodOf($ancestorFacts, $name) !== null) {
                    $searchNames[] = $ancestor;
                }
            }
        }
        $shortNames = array_values(array_unique(array_map(fn (string $class) => strtolower(self::shortName($class)), $searchNames)));
        if ($isConstructor) {
            foreach ([...$shortNames, '__construct'] as $word) {
                $this->ensureScanned($word, false);
            }
        } else {
            $this->ensureScanned($name, false);
        }

        $paths = [];
        if ($isConstructor) {
            foreach ($family as $member) {
                $paths += $this->constructions[strtolower($member)] ?? [];
            }
        } else {
            $paths = $this->pathsByName[$name] ?? [];
        }
        if (! $isConstructor) {
            $this->ensureScanned('(->|::)\\s*\\$\\w+\\s*\\(', true);
            $paths += $this->pathsByName[self::DYNAMIC_NAME] ?? [];
        }
        $sites = [];
        $receiverTypes = [];
        $this->recordUse('name', $name);
        $this->recordUse('name', self::DYNAMIC_NAME);
        foreach (array_keys($paths) as $path) {
            $this->recordUse('path', $path);
            foreach ([...$this->sitesCalled($path, $name), ...($isConstructor ? [] : $this->sitesCalled($path, self::DYNAMIC_NAME))] as $site) {
                if ($site[2] !== 'function') {
                    $sites[] = [$path, $site];
                    array_push($receiverTypes, ...($site[3] ?? []));
                }
            }
        }
        $this->ensureClasses($receiverTypes);

        $edges = [];
        foreach ($sites as [$path, $site]) {
            [, $line, $kind, $types, $receiverConfidence] = $site;
            // A docblock type naming no class (a template parameter like `TModel`, a conditional type) says nothing.
            if ($types !== null && array_filter($types, fn (string $type) => isset($this->classFacts[strtolower(ltrim($type, '\\'))])) === []) {
                $types = null;
            }
            if ($types === null) {
                // `$x::m()` is rare and always dynamic; `$x->m()` is common, so only where the class is named.
                if ($kind === 'late static' || (($kind === 'call' || $kind === 'dynamic') && $this->mentionsAny($path, $shortNames))) {
                    $edges[] = $this->edge($path, $site, 'unresolved');
                }

                continue;
            }
            if ($kind === 'dynamic') {
                if ($this->sameFamily($types, $target)) {
                    $edges[] = $this->edge($path, $site, 'unresolved');
                }

                continue;
            }
            $stored = $kind === 'stored callable';
            $best = null;
            foreach ($types as $type) {
                $match = $this->matchType($type, $target, $name, $isConstructor, $targetDeclaring, $kind === 'static', $targetAbstract);
                if ($match === 'exact') {
                    $best = 'exact';
                    break;
                }
                $best ??= $match;
            }
            if ($best !== null) {
                $edges[] = $this->edge($path, $site, $stored ? 'unresolved' : ($best === 'exact' && $receiverConfidence !== 'exact' ? 'likely' : $best));
            }
        }

        return $edges;
    }

    /**
     * `__call` runs for a method an object's class does not declare (`__callStatic` for a static call): every
     * call on the class (or a subclass not declaring that name) whose method it lacks.
     */
    private function magicCallers(array $target, bool $static): array
    {
        $family = [$target['name'], ...$this->subclasses($target['name'])];
        foreach ($family as $member) {
            $this->ensureScanned(self::shortName($member), false);
        }
        $familyKeys = array_flip(array_map('strtolower', $family));
        // A site's receiver type has class facts only under its own lowercase name, so only files with a call on a
        // member of the family by name can hold one; the answer still depends on every file.
        $candidates = [];
        foreach (array_keys($familyKeys) as $key) {
            $candidates += $this->pathsByReceiverType[$key] ?? [];
        }
        $edges = [];
        foreach (array_keys($this->fileFacts) as $path) {
            $this->recordUse('path', $path);
            if (! isset($candidates[$path])) {
                continue;
            }
            foreach ($this->allSites($path) as $called => $sites) {
                foreach ($sites as $site) {
                    $isStatic = $site[2] === 'static' || $site[2] === 'late static';
                    if ($called === self::DYNAMIC_NAME || $site[3] === null || $isStatic !== $static || ! in_array($site[2], ['call', 'static', 'late static'], true)) {
                        continue;
                    }
                    $this->ensureClasses($site[3]);
                    foreach ($site[3] as $type) {
                        $typeFacts = $this->classFacts[strtolower(ltrim($type, '\\'))] ?? null;
                        if ($typeFacts !== null && isset($familyKeys[strtolower($typeFacts['name'])]) && $this->methodOf($typeFacts, $called) === null) {
                            $edges[] = $this->edge($path, $site, 'likely');
                            break;
                        }
                    }
                }
            }
        }

        return $edges;
    }

    /**
     * A method of `new class extends A implements B {}` is reached only through what it overrides or implements:
     * every caller of `A::m` or `B::m` may run it.
     */
    private function anonymousClassCallers(string $class, string $method): array
    {
        $parents = str_contains($class, '@anonymous_') ? ($this->declaredClasses[strtolower($class)][2] ?? []) : [];
        $this->ensureClasses($parents);
        $edges = [];
        foreach ($parents as $parent) {
            $facts = $this->classFacts[strtolower($parent)] ?? null;
            $declared = $facts === null ? null : $this->methodOf($facts, strtolower($method));
            foreach ($declared === null ? [] : $this->computeCallers("{$facts['name']}::{$declared[1]}") as $edge) {
                $edges[] = [...$edge, 'confidence' => $edge['confidence'] === 'exact' ? 'likely' : $edge['confidence']];
            }
        }

        return $edges;
    }

    /** Whether an object of one of $types may be of the target's class (it, an ancestor or a descendant of it). */
    private function sameFamily(array $types, array $target): bool
    {
        $targetKey = strtolower($target['name']);
        foreach ($types as $type) {
            $facts = $this->classFacts[strtolower(ltrim($type, '\\'))] ?? null;
            $typeKey = strtolower($facts['name'] ?? '');
            if ($facts !== null && ($typeKey === $targetKey || isset($facts['ancestorKeys'][$targetKey]) || isset($target['ancestorKeys'][$typeKey]))) {
                return true;
            }
        }

        return false;
    }

    private function mentionsAny(string $path, array $shortNames): bool
    {
        foreach ($shortNames as $shortName) {
            if (isset($this->fileFacts[$path]['mentions'][$shortName])) {
                return true;
            }
        }

        return false;
    }

    /**
     * The method $class has under this lowercase name, the way PHP resolves it: declared in the class itself
     * (traits included), else in the nearest parent, else in an interface.
     *
     * @return array{0: string, 1: string, 2: ?string, 3: int, 4: ?string}|null
     */
    private function methodOf(array $classFacts, string $method): ?array
    {
        if (isset($classFacts['methods'][$method])) {
            return $classFacts['methods'][$method];
        }
        $this->ensureClasses($classFacts['ancestors']);
        foreach ($classFacts['ancestors'] as $ancestor) {
            $declared = $this->classFacts[strtolower($ancestor)]['methods'][$method] ?? null;
            if ($declared !== null) {
                return $declared;
            }
        }

        return null;
    }

    /**
     * Where a method's code is written: `Trait::m` for a method a class takes from a trait (reflection names the
     * class as declaring it), so the trait method and the same method seen on every class using it are one.
     */
    private static function methodSource(array $declared): string
    {
        return strtolower($declared[4] ?? "{$declared[0]}::{$declared[1]}");
    }

    /**
     * @param bool $fixed a call that names its class (`parent::m()`): it never reaches an override in a subclass
     * @param bool $abstract the target method has no body, so every implementation of it in the family counts
     */
    private function matchType(string $type, array $target, string $method, bool $isConstructor, string $targetDeclaring, bool $fixed = false, bool $abstract = false, bool $followMixins = true): ?string
    {
        $facts = $this->classFacts[strtolower(ltrim($type, '\\'))] ?? null;
        if ($facts === null) {
            return null;
        }
        $typeKey = strtolower($facts['name']);
        $targetKey = strtolower($target['name']);
        if ($typeKey === $targetKey || isset($facts['ancestorKeys'][$targetKey])) {
            $declared = $this->methodOf($facts, $method);
            if ($isConstructor) {
                return $typeKey === $targetKey || ($declared !== null && self::methodSource($declared) === $targetDeclaring) ? 'exact' : null;
            }
            if ($declared === null) {
                return $facts['magic'] ? 'likely' : null;
            }

            return $abstract || self::methodSource($declared) === $targetDeclaring ? 'exact' : null;
        }
        // In a trait, `self::m()` runs as the class using it: any class of the target's family may be that one.
        if ((! $fixed || ! empty($facts['trait'])) && ! $isConstructor && isset($target['ancestorKeys'][$typeKey]) && $this->methodOf($facts, $method) !== null) {
            return 'likely';
        }
        // A method the class lacks goes to its `__call`, which hands it on to what `@mixin` names (one step: mixins
        // may name each other).
        if ($followMixins && $facts['magic'] && ! $isConstructor && ($facts['mixins'] ?? []) !== [] && $this->methodOf($facts, $method) === null) {
            $this->ensureClasses($facts['mixins']);
            foreach ($facts['mixins'] as $mixin) {
                if ($this->matchType($mixin, $target, $method, false, $targetDeclaring, $fixed, $abstract, false) !== null) {
                    return 'likely';
                }
            }
        }

        return null;
    }

    private function edge(string $path, array $site, string $confidence): array
    {
        return ['from' => $this->scopeRef($path, $site[5]), 'at' => ['file' => $path, 'line' => $site[1]], 'kind' => $site[2], 'confidence' => $confidence];
    }

    private function scopeRef(string $path, int $scopeIndex): array
    {
        $scope = $this->scopes($path)[$scopeIndex] ?? null;
        $isTest = $this->files->isTestPath($path);
        if ($scope === null) {
            return ['symbol' => $path, 'file' => $path, 'line' => 1, 'kind' => $isTest ? 'test' : 'file'];
        }
        $ref = ['symbol' => $scope[0], 'file' => $path, 'line' => $scope[1]];
        // A closure at the top of a file runs when the file is loaded, like the rest of its top-level code.
        $topLevelClosure = str_contains($scope[0], ':') && ! str_contains($scope[0], '::');
        if ($scope[4] || $isTest) {
            $ref['kind'] = 'test';
        } elseif ($topLevelClosure) {
            $ref['kind'] = 'file';
        }

        return $ref;
    }

    private function functionCallers(string $function): array
    {
        $target = strtolower(ltrim($function, '\\'));
        $short = strtolower(self::shortName($target));
        $this->ensureScanned($short, false);
        $edges = [];
        $this->recordUse('name', $short);
        foreach (array_keys($this->pathsByName[$short] ?? []) as $path) {
            $this->recordUse('path', $path);
            foreach ($this->sitesCalled($path, $short) as $site) {
                if ($site[2] === 'function' && in_array($target, $site[3], true)) {
                    $edges[] = $this->edge($path, $site, $site[3][0] === $target ? 'exact' : 'likely');
                }
            }
        }

        return $edges;
    }

    /** `Fqcn::method`, `ShortClass::method`, `Fqcn`, `ShortClass` or a function name. */
    public function resolve(string $query): array
    {
        $started = microtime(true);
        $result = $this->resolveSymbols($query);
        profilePhase('resolve', $started, ['query' => $query]);

        return $result;
    }

    private function resolveSymbols(string $query): array
    {
        $query = ltrim(trim($query), '\\');
        [$classPart, $method] = str_contains($query, '::') ? explode('::', $query, 2) : [$query, null];

        $classes = [];
        if ($this->classFactsFor($classPart) !== null) {
            $classes[] = $this->classFacts[strtolower($classPart)]['name'];
        } elseif (! str_contains($classPart, '\\') && isIdentifierText($classPart)) {
            $this->ensureScanned('(class|interface|trait|enum)\\s+'.preg_quote($classPart).'\\b', true);
            $short = strtolower($classPart);
            $matching = [];
            foreach ($this->declaredClasses as [$declaredName]) {
                if (strtolower(self::shortName($declaredName)) === $short) {
                    $matching[] = $declaredName;
                }
            }
            $this->ensureClasses($matching);
            foreach ($matching as $class) {
                if (isset($this->classFacts[strtolower($class)])) {
                    $classes[] = $this->classFacts[strtolower($class)]['name'];
                }
            }
        }

        $symbols = [];
        foreach (array_unique($classes) as $class) {
            $facts = $this->classFacts[strtolower($class)];
            $declared = $method === null ? null : $this->methodOf($facts, strtolower($method));
            if ($method === null || ($declared === null && strcasecmp($method, '__construct') === 0)) {
                $symbols[] = ['symbol' => $facts['name'], 'file' => $facts['file'], 'line' => $this->currentLine($facts['file'], $facts['name'], $facts['line'])];
            } elseif ($declared !== null) {
                $symbols[] = ['symbol' => "{$facts['name']}::{$declared[1]}", 'file' => $declared[2], 'line' => $this->currentLine($declared[2], $declared[4] ?? "{$declared[0]}::{$declared[1]}", $declared[3])];
            }
        }
        // A name matching a class only by case (`setting` for `Setting`) also means the function or methods of that
        // exact name.
        $caseOnlyClass = $classes !== [] && ! in_array($query, array_map(fn (string $class) => self::shortName($class), $classes), true);
        if (($symbols === [] || $caseOnlyClass) && $method === null) {
            $function = $this->pool->map('function', [$query], 1)[0];
            if ($function !== null) {
                $symbols[] = $function;
            }
        }
        // A bare method name (`shouldSendMessage`): every method of that name, as a text search would find it.
        if (($symbols === [] || $caseOnlyClass) && $method === null && isIdentifierText($query)) {
            $this->ensureScanned($query, false);
            $wanted = strtolower($query);
            foreach (array_keys($this->pathsByMethodName[$wanted] ?? []) as $path) {
                foreach ($this->scopes($path) as $scope) {
                    if (self::declaredMethodName($scope) === $wanted) {
                        $symbols[] = ['symbol' => $scope[0], 'file' => $path, 'line' => $scope[1]];
                    }
                }
            }
        }

        return $symbols;
    }

    /**
     * Where a declaration starts now. Class facts are kept while a class's shape is unchanged, and that shape
     * deliberately ignores positions, so lines come from the file's facts, which are re-read on every change.
     */
    private function currentLine(?string $path, string $symbol, int $recordedLine): int
    {
        if ($path === null) {
            return $recordedLine;
        }
        $this->extractFiles([$path]);
        foreach ($this->scopes($path) as $scope) {
            if (strcasecmp($scope[0], $symbol) === 0) {
                return $scope[1];
            }
        }

        return $recordedLine;
    }

    public function locate(string $path, int $line): ?array
    {
        $this->extractFiles([$path]);
        $facts = $this->facts($path);
        if ($facts === null) {
            return null;
        }
        $innermost = -1;
        foreach ($facts['scopes'] as $index => $scope) {
            if ($line >= $scope[1] && $line <= $scope[2] && ($innermost === -1 || $scope[1] >= $facts['scopes'][$innermost][1])) {
                $innermost = $index;
            }
        }

        return $this->scopeRef($path, $innermost);
    }
}
