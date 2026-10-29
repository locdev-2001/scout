<?php

declare(strict_types=1);

/**
 * The driver process stays small and long-lived: it loads only the Composer autoloader and keeps facts.
 * Anything that needs the project's classes runs in a child forked from it, because PHP cannot unload or
 * reload a class inside one process. With OPcache on, children share compiled files through shared memory
 * and revalidate them against the disk on every include, so each child sees the code as it is now.
 */
/**
 * Values passed between processes and saved to disk. In igbinary's format when PHP has the extension: it stores a
 * repeated string once and reads it back as one shared string, and facts repeat class, method and file names, so
 * they take about half the memory and read twice as fast. Else in PHP's own format.
 */
final class Payload
{
    public static function encode(mixed $value): string
    {
        return function_exists('igbinary_serialize') ? igbinary_serialize($value) : serialize($value);
    }

    /** False when the bytes hold nothing this PHP can read. */
    public static function decode(string $bytes, bool $objects = true): mixed
    {
        // igbinary's format starts with a zero byte; PHP's never does.
        if (str_starts_with($bytes, "\0")) {
            return function_exists('igbinary_unserialize') ? @igbinary_unserialize($bytes) : false;
        }

        return @unserialize($bytes, ['allowed_classes' => $objects]);
    }
}

final class ForkedWorker
{
    /**
     * Without fork (PHP on Windows has no pcntl), work runs in the driver itself: the classes it loads stay
     * loaded, so EngineGuard restarts the driver when any of their files changes. Slower after an edit, never stale.
     */
    public static function inline(): bool
    {
        static $inline = null;

        return $inline ??= ! function_exists('pcntl_fork') || getenv('SCOUT_NO_FORK') === '1';
    }

    /**
     * Runs $work here and returns a finished job, read like a child's answer.
     *
     * @return array{0: int, 1: resource}
     */
    public static function runHere(callable $work, string $label = 'run'): array
    {
        $started = microtime(true);
        try {
            $payload = ['result' => $work()];
            profilePhase("inline: {$label}", $started);
        } catch (Throwable $error) {
            $payload = ['error' => get_class($error).': '.$error->getMessage()];
        }
        $stream = fopen('php://memory', 'w+');
        fwrite($stream, Payload::encode($payload));
        rewind($stream);

        return [0, $stream];
    }

    /**
     * Runs $work over $items split into chunks, in up to $parallelism children at once.
     *
     * @return list<mixed> one result per chunk, in chunk order
     */
    public static function map(array $items, int $chunkSize, int $parallelism, callable $work): array
    {
        $chunks = array_chunk($items, max(1, $chunkSize));
        $results = [];
        foreach (array_chunk($chunks, max(1, $parallelism), true) as $wave) {
            $running = [];
            foreach ($wave as $index => $chunk) {
                $running[$index] = self::start(fn () => $work($chunk));
            }
            foreach ($running as $index => $child) {
                $results[$index] = self::collect($child);
            }
        }
        ksort($results);

        return array_values($results);
    }

    public static function run(callable $work, string $label = 'run'): mixed
    {
        return self::collect(self::start($work, $label));
    }

    /**
     * Starts $work in a child, at the lowest CPU priority unless said otherwise, and returns at once; feed the stream to
     * readAvailable() when it is readable until it returns the result.
     *
     * @return array{0: int, 1: resource, 2: string}
     */
    public static function startBackground(callable $work, string $label = 'background', bool $lowPriority = true, bool $takesSlot = false): array
    {
        [$pid, $stream] = self::start(function () use ($work, $lowPriority, $takesSlot) {
            if ($lowPriority && ! self::inline() && function_exists('proc_nice')) {
                proc_nice(19);
            }
            if ($takesSlot && ! self::inline()) {
                WorkSlot::take();
            }

            return $work();
        }, $label);
        stream_set_blocking($stream, false);

        return [$pid, $stream, ''];
    }

    /**
     * Reads what a background child has written so far; returns [true, result] once it finished.
     *
     * @param  array{0: int, 1: resource, 2: string}  $job
     * @return array{0: bool, 1: mixed}
     */
    public static function readAvailable(array &$job): array
    {
        while (($chunk = fread($job[1], 1 << 20)) !== false && $chunk !== '') {
            $job[2] .= $chunk;
        }
        if (! feof($job[1])) {
            return [false, null];
        }
        fclose($job[1]);
        if ($job[0] > 0) {
            self::$unreaped[] = $job[0];
        }
        $payload = $job[2] === '' ? null : Payload::decode($job[2]);

        return [true, is_array($payload) ? ($payload['result'] ?? null) : null];
    }

    /** A child let go without being waited for: reaped with the finished ones. */
    public static function released(int $pid): void
    {
        self::$unreaped[] = $pid;
    }

    /** Whether children finished or let go still wait to be reaped. */
    public static function hasUnreaped(): bool
    {
        return self::$unreaped !== [];
    }

    public static function reapFinished(): void
    {
        if (self::inline()) {
            return;
        }
        foreach (self::$unreaped as $index => $pid) {
            if (pcntl_waitpid($pid, $status, WNOHANG) !== 0) {
                unset(self::$unreaped[$index]);
            }
        }
    }

    /** @return array{0: int, 1: resource} */
    private static function start(callable $work, string $label = 'run'): array
    {
        if (self::inline()) {
            return self::runHere($work, $label);
        }
        self::reapFinished();
        [$parentEnd, $childEnd] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
        $pid = pcntl_fork();
        if ($pid === -1) {
            throw new RuntimeException('cannot fork: '.pcntl_strerror(pcntl_get_last_error()));
        }
        if ($pid === 0) {
            fclose($parentEnd);
            gc_disable();
            $started = microtime(true);
            try {
                $payload = ['result' => $work()];
                profilePhase("worker: {$label}", $started);
            } catch (Throwable $error) {
                $payload = ['error' => get_class($error).': '.$error->getMessage()];
            }
            fwrite($childEnd, Payload::encode($payload));
            fclose($childEnd);
            // Skip shutdown functions and destructors inherited from the parent (an app may register its own).
            posix_kill(getmypid(), SIGKILL);
        }
        fclose($childEnd);

        return [$pid, $parentEnd];
    }

    private static function collect(array $child): mixed
    {
        return self::finish($child[0], $child[1]);
    }

    /** A child that answers nothing and uses no CPU for this long waits for something that will not come. */
    private const STUCK_SECONDS = 10;

    /** Where CPU time cannot be read, a child writes only when done: silence alone means stuck only after this. */
    private const SILENT_SECONDS = 300;

    /**
     * Everything a child writes until it closes its end. A child that stopped working (it waits on a lock or a
     * network peer) is ended, so one question fails instead of the whole driver hanging.
     *
     * @param  resource  $stream
     */
    private static function readUnlessStuck(int $pid, $stream): string
    {
        $data = '';
        $ticks = self::cpuTicks($pid);
        $quietSince = microtime(true);
        while (! feof($stream)) {
            $read = [$stream];
            $write = $except = null;
            if (@stream_select($read, $write, $except, 1) > 0) {
                $data .= (string) fread($stream, 1 << 20);
                $quietSince = microtime(true);

                continue;
            }
            $now = self::cpuTicks($pid);
            if ($now !== $ticks) {
                [$ticks, $quietSince] = [$now, microtime(true)];
            } elseif ($pid > 0 && microtime(true) - $quietSince > ($ticks === null ? self::SILENT_SECONDS : self::STUCK_SECONDS)) {
                posix_kill($pid, SIGKILL);
                throw new RuntimeException('worker gave no answer and did no work for '.round(microtime(true) - $quietSince).' s');
            }
        }

        return $data;
    }

    /** CPU time a process used so far (Linux, macOS); null where it cannot be read, so only silence counts there. */
    private static function cpuTicks(int $pid): ?int
    {
        $stat = $pid > 0 ? @file_get_contents("/proc/{$pid}/stat") : false;
        if ($stat === false && $pid > 0 && PHP_OS_FAMILY === 'Darwin') {
            // No /proc: `ps` once a second, only while waiting on a quiet child. `[[hh:]mm:]ss.cc`, in hundredths.
            $time = trim((string) shell_exec("ps -o cputime= -p {$pid} 2>/dev/null"));
            if (preg_match('/^(?:(\d+):)?(\d+):(\d+)\.(\d+)$/', $time, $match) !== 1) {
                return null;
            }

            return ((((int) $match[1] * 60 + (int) $match[2]) * 60 + (int) $match[3]) * 100) + (int) substr($match[4].'0', 0, 2);
        }
        if ($stat === false) {
            return null;
        }
        $fields = explode(' ', substr($stat, strrpos($stat, ')') + 2));

        return (int) $fields[11] + (int) $fields[12];
    }

    /** @var list<int> children that answered but were not waited for yet */
    private static array $unreaped = [];

    /**
     * Reads a child's answer. The child is not waited for here: tearing down a copy of a large process takes
     * as long as forking it, so reaping happens later, between requests.
     *
     * @param  resource  $stream
     */
    public static function finish(int $pid, $stream): mixed
    {
        // A worker ended for being stuck is closed and reaped like one that answered.
        try {
            $data = self::readUnlessStuck($pid, $stream);
        } finally {
            fclose($stream);
            if ($pid > 0) {
                self::$unreaped[] = $pid;
            }
        }
        $payload = $data === '' ? null : Payload::decode($data);
        if (! is_array($payload)) {
            throw new RuntimeException('worker died without an answer');
        }
        if (isset($payload['error'])) {
            throw new RuntimeException($payload['error']);
        }

        return $payload['result'];
    }
}

/**
 * One of the machine-wide work slots the core hands out (`scout hold-slot`), so repositories reading their code at
 * the same moment share the machine. Taken by a forked child before heavy work; held until that child ends, when the
 * helper sees its stdin close and the slot is free again.
 */
final class WorkSlot
{
    /** @var array{0: resource, 1: array<int, resource>}|null */
    private static ?array $held = null;

    /** Waits for a slot; returns at once when the core hands out none (slots turned off, or no helper). */
    public static function take(): void
    {
        $helper = getenv('SCOUT_SLOT_HELPER');
        if (self::$held !== null || $helper === false || $helper === '') {
            return;
        }
        $process = proc_open([$helper, 'hold-slot'], [0 => ['pipe', 'r'], 1 => ['pipe', 'w']], $pipes);
        if (! is_resource($process)) {
            return;
        }
        fgets($pipes[1]);
        self::$held = [$process, $pipes];
    }
}

/**
 * Children forked ahead of time, while the driver is idle, so a query does not pay for forking a large process.
 * Each one waits for a single task, loads whatever classes that task needs from the current files (it has loaded
 * none before), answers and is discarded. Tasks are named kinds with plain data, because a closure created after
 * a child was forked does not exist in that child.
 */
final class WorkerPool
{
    /** @var list<array{0: int, 1: resource}> */
    private array $idle = [];

    /** @param Closure(string, array): mixed $dispatch runs one task in the child */
    public function __construct(private readonly int $size, private readonly Closure $dispatch) {}

    /**
     * Workers kept forked while idle. One answers a single-file question at once; a question that needs more forks
     * them on demand (they share the parent's memory until they write), so an idle driver does not hold a full set.
     */
    private const IDLE_WORKERS = 1;

    public function needsWorkers(): bool
    {
        return ! ForkedWorker::inline() && count($this->idle) < min($this->size, self::IDLE_WORKERS);
    }

    /**
     * Lets the idle workers go (each reads the end of its stream and exits), so the next ones fork from the driver
     * as it is now: one forked during the warm-up keeps its own copy of every page the driver wrote since, which
     * grows to most of the driver's memory while it waits.
     */
    public function renewIdle(): void
    {
        foreach ($this->idle as [$pid, $stream]) {
            fclose($stream);
            ForkedWorker::released($pid);
        }
        $this->idle = [];
    }

    /** Reaps finished children and forks one idle worker; called only while no request is waiting. */
    public function addOne(): void
    {
        ForkedWorker::reapFinished();
        if ($this->needsWorkers()) {
            $this->idle[] = $this->spawn();
        }
    }

    /**
     * Runs task $kind over $items split into chunks, at most the pool size at once.
     *
     * @return list<mixed> one result per chunk, in chunk order
     */
    public function map(string $kind, array $items, int $chunkSize): array
    {
        $results = [];
        foreach (array_chunk(array_chunk($items, max(1, $chunkSize)), $this->size, true) as $wave) {
            $running = [];
            foreach ($wave as $index => $chunk) {
                if (ForkedWorker::inline()) {
                    $running[$index] = ForkedWorker::runHere(fn () => ($this->dispatch)($kind, $chunk), "pool {$kind}");

                    continue;
                }
                $worker = array_shift($this->idle) ?? $this->spawn();
                $message = Payload::encode([$kind, $chunk]);
                fwrite($worker[1], pack('J', strlen($message)).$message);
                $running[$index] = $worker;
            }
            try {
                foreach ($running as $index => [$pid, $stream]) {
                    unset($running[$index]);
                    $results[$index] = ForkedWorker::finish($pid, $stream);
                }
            } catch (RuntimeException $error) {
                // The rest of the wave is let go (and reaped later), not left to linger as zombies.
                foreach ($running as [$pid, $stream]) {
                    fclose($stream);
                    ForkedWorker::released($pid);
                }
                throw $error;
            }
        }
        ksort($results);

        return array_values($results);
    }

    /** @return array{0: int, 1: resource} */
    private function spawn(): array
    {
        // A long question forks many one-shot workers: without this, the ones already gone pile up as zombies
        // until it ends and can reach the per-user process limit.
        ForkedWorker::reapFinished();
        [$parentEnd, $childEnd] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
        $parent = getmypid();
        $pid = pcntl_fork();
        if ($pid === -1) {
            throw new RuntimeException('cannot fork: '.pcntl_strerror(pcntl_get_last_error()));
        }
        if ($pid === 0) {
            // Drop inherited streams (other workers' sockets, the warm-up's), so no process outlives its peer.
            foreach (get_resources('stream') as $stream) {
                if ($stream !== $childEnd && $stream !== STDIN && $stream !== STDOUT && $stream !== STDERR) {
                    @fclose($stream);
                }
            }
            $this->serveOnce($childEnd, $parent);
        }
        fclose($childEnd);

        return [$pid, $parentEnd];
    }

    /** @param resource $stream */
    private function serveOnce($stream, int $parent): never
    {
        do {
            $read = [$stream];
            $write = $except = null;
            if (posix_getppid() !== $parent) {
                posix_kill(getmypid(), SIGKILL);
            }
        } while (@stream_select($read, $write, $except, 1) === 0);
        $header = fread($stream, 8);
        $length = strlen((string) $header) === 8 ? unpack('J', $header)[1] : 0;
        $message = '';
        while (strlen($message) < $length && ! feof($stream)) {
            $message .= fread($stream, $length - strlen($message));
        }
        if ($length === 0 || strlen($message) < $length) {
            posix_kill(getmypid(), SIGKILL);
        }
        clearstatcache(true);
        // A short-lived child: the cycle collector would walk (and so copy) the parent's inherited heap.
        gc_disable();
        $started = microtime(true);
        try {
            $task = Payload::decode($message);
            $payload = ['result' => ($this->dispatch)(...$task)];
            profilePhase("worker: pool {$task[0]}", $started);
        } catch (Throwable $error) {
            $payload = ['error' => get_class($error).': '.$error->getMessage()];
        }
        fwrite($stream, Payload::encode($payload));
        fclose($stream);
        posix_kill(getmypid(), SIGKILL);
        exit(1);
    }
}

/**
 * Keeps the long-lived driver honest: OPcache entries of changed files are dropped (it compares mtime only,
 * so a write that restores an old mtime would otherwise be missed), and the driver exits when a file it
 * loaded itself changed, so the core restarts it on current code.
 */
final class EngineGuard
{
    /** @var array<string, array{0: int, 1: int, 2: int}> */
    private array $loadedFiles = [];

    public function __construct(private readonly string $repoRoot)
    {
        foreach (get_included_files() as $path) {
            $this->loadedFiles[$path] = self::stamp($path);
        }
    }

    private static function stamp(string $path): array
    {
        $stat = @stat($path);

        return $stat === false ? [0, 0, 0] : [$stat['mtime'], $stat['ctime'], $stat['ino']];
    }

    /** @param list<string> $changedPaths repo-relative paths reported by the core */
    public function apply(array $changedPaths, array $restartInputs): void
    {
        clearstatcache();
        foreach ($changedPaths as $path) {
            $absolute = $this->repoRoot.'/'.$path;
            if (function_exists('opcache_invalidate')) {
                opcache_invalidate($absolute, true);
            }
            forgetCompiled($absolute);
            if (in_array($path, $restartInputs, true)) {
                self::restart("{$path} changed");
            }
        }
        if (ForkedWorker::inline()) {
            $loaded = array_flip(get_included_files());
            foreach ($changedPaths as $path) {
                $absolute = $this->repoRoot.'/'.$path;
                if (isset($loaded[realpath($absolute) ?: $absolute])) {
                    self::restart("{$path}, loaded into the driver (no fork on this system), changed");
                }
            }
        }
        foreach ($this->loadedFiles as $path => $stamp) {
            if (self::stamp($path) !== $stamp) {
                self::restart("{$path}, loaded by the driver itself, changed");
            }
        }
    }

    public static function restart(string $reason): never
    {
        fwrite(STDERR, "restarting: {$reason}\n");
        exit(0);
    }
}

function workerParallelism(): int
{
    // macOS has no `nproc`; `getconf` answers there and on Linux.
    $cores = (int) (shell_exec('nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null') ?: 2);

    return max(1, intdiv($cores, 2));
}

/** Appends one timing line to the file named by SCOUT_PROFILE, when set; used to measure, never to answer. */
function profilePhase(string $phase, float $startedAt, array $details = []): void
{
    $target = getenv('SCOUT_PROFILE');
    if ($target === false || $target === '') {
        return;
    }
    $line = sprintf("%s %-28s %7.1f ms %s\n", basename(dirname($_SERVER['SCRIPT_FILENAME'] ?? '')), $phase, (microtime(true) - $startedAt) * 1000, $details === [] ? '' : json_encode($details));
    file_put_contents($target, $line, FILE_APPEND);
}
