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
    require_once $autoload;
}
