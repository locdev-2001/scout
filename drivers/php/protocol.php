<?php

declare(strict_types=1);

/** Work a driver runs between requests (for example a low-priority warm-up), fed by readable streams. */
interface BackgroundWork
{
    /** @return list<resource> */
    public function streams(): array;

    /** @param resource $stream */
    public function onReadable($stream): void;

    /** Whether idleStep() has something to do. */
    public function hasIdleWork(): bool;

    /** One small unit of work done only when no request has arrived for a while (for example one fork). */
    public function idleStep(): void;
}

/** A query sends several requests back to back; idle work waits for this long a quiet moment. */
const IDLE_AFTER_SECONDS = 0.005;

/**
 * JSON lines over stdio: one request `{id, op, ...}` per line in, one `{id, result | error}` per line out.
 * An op the driver does not handle answers null, which the core reads as "nothing from this driver".
 * While no request is waiting, background work is fed whenever one of its streams has data.
 *
 * @param array<string, callable(array): mixed> $handlers
 */
function serveDriver(array $handlers, ?BackgroundWork $background = null): void
{
    while (true) {
        $read = [STDIN, ...($background?->streams() ?? [])];
        $write = null;
        $except = null;
        $idleWork = $background?->hasIdleWork() ?? false;
        $ready = @stream_select($read, $write, $except, $idleWork ? 0 : null, $idleWork ? (int) (IDLE_AFTER_SECONDS * 1e6) : null);
        if ($ready === 0) {
            $background->idleStep();

            continue;
        }
        if ($ready === false) {
            continue;
        }
        foreach ($read as $stream) {
            if ($stream !== STDIN) {
                $background->onReadable($stream);
            }
        }
        if (! in_array(STDIN, $read, true)) {
            continue;
        }
        // fgets may pull several request lines into PHP's buffer, which select() does not see: drain it.
        do {
            $line = fgets(STDIN);
            if ($line === false) {
                return;
            }
            answerRequest($handlers, $line);
        } while (stream_get_meta_data(STDIN)['unread_bytes'] > 0);
    }
}

/** @param array<string, callable(array): mixed> $handlers */
function answerRequest(array $handlers, string $line): void
{
    $request = json_decode($line, true);
    $id = $request['id'] ?? null;
    try {
        $handler = $handlers[$request['op'] ?? ''] ?? null;
        $response = ['id' => $id, 'result' => $handler ? $handler($request) : null];
    } catch (Throwable $error) {
        $response = ['id' => $id, 'error' => get_class($error).': '.$error->getMessage()];
    }
    fwrite(STDOUT, json_encode($response, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_INVALID_UTF8_SUBSTITUTE)."\n");
}

/** The file Composer would load for a class, found without loading it (the long-lived driver must not). */
function composerClassFile(string $class): ?string
{
    foreach (Composer\Autoload\ClassLoader::getRegisteredLoaders() as $loader) {
        $file = $loader->findFile(ltrim($class, '\\'));
        if (is_string($file)) {
            return realpath($file) ?: $file;
        }
    }

    return null;
}

function loadComposerAutoload(string $repoRoot): void
{
    $autoload = $repoRoot.'/vendor/autoload.php';
    if (! is_file($autoload)) {
        throw new RuntimeException("missing $autoload: run composer install");
    }
    $loader = require_once $autoload;
    $sharedRoot = sharedCheckoutRoot($repoRoot);
    if ($sharedRoot === null || ! $loader instanceof Composer\Autoload\ClassLoader) {
        return;
    }
    // PHP resolves the link, so Composer would load the repository's own classes from the other checkout: another
    // branch's code. They are loaded from this one.
    $here = fn (string $path) => checkoutPath($path, $sharedRoot, $repoRoot);
    foreach ($loader->getPrefixesPsr4() as $prefix => $paths) {
        $loader->setPsr4($prefix, array_map($here, $paths));
    }
    foreach ($loader->getPrefixes() as $prefix => $paths) {
        $loader->set($prefix, array_map($here, $paths));
    }
    $loader->addClassMap(array_map($here, $loader->getClassMap()));
}

/**
 * The checkout whose vendor/ this repository's vendor/ links to (a git worktree sharing the main checkout's
 * packages), or null when vendor/ is its own.
 */
function sharedCheckoutRoot(string $repoRoot): ?string
{
    static $roots = [];
    if (! array_key_exists($repoRoot, $roots)) {
        $vendor = realpath($repoRoot.'/vendor');
        $roots[$repoRoot] = $vendor !== false && $vendor !== $repoRoot.'/vendor' ? dirname($vendor) : null;
    }

    return $roots[$repoRoot];
}

/** A path into the shared checkout's own code (not its packages) as the same path in this repository. */
function checkoutPath(string $path, string $sharedRoot, string $repoRoot): string
{
    $parts = [];
    foreach (explode('/', $path) as $index => $part) {
        if ($part === '..') {
            array_pop($parts);
        } elseif ($part !== '.' && ($part !== '' || $index === 0)) {
            $parts[] = $part;
        }
    }
    $normalized = implode('/', $parts);

    return str_starts_with($normalized, $sharedRoot.'/') && ! str_starts_with($normalized, $sharedRoot.'/vendor/') ? $repoRoot.substr($normalized, strlen($sharedRoot)) : $path;
}
