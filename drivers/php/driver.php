<?php

declare(strict_types=1);

require __DIR__.'/protocol.php';
require __DIR__.'/worker.php';
require __DIR__.'/php-source.php';
require __DIR__.'/type-inference.php';
require __DIR__.'/fact-extraction.php';
require __DIR__.'/fact-store.php';

$repoRoot = rtrim(realpath($argv[1] ?? '') ?: '', '/');
loadComposerAutoload($repoRoot);

/** What a pooled child can be asked to do; defined before any child is forked, so every child has it. */
$tasks = function (string $kind, array $items) use ($repoRoot): mixed {
    $extraction = new FactExtraction(new PhpFileCache($repoRoot));

    return match ($kind) {
        'files' => array_combine(array_column($items, 0), array_map(fn (array $item) => $extraction->fileFacts(...$item), $items)),
        'classes' => array_map([$extraction, 'classFacts'], $items),
        'answers' => $extraction->answers($items),
        'refresh' => array_combine(array_column($items, 0), array_map(fn (array $job) => $extraction->refreshSites($job[0], $job[1]), $items)),
        'function' => $extraction->functionFacts($items[0]),
    };
};
$parallelism = workerParallelism();
$store = new FactStore(new PhpFileCache($repoRoot), new EngineGuard($repoRoot), new WorkerPool($parallelism, $tasks), $parallelism);

$store->startWarmUp();

serveDriver([
    'probe' => fn () => $store->probe(),
    'sync' => fn (array $request) => $store->sync($request['changed'] ?? []),
    'resolve' => fn (array $request) => $store->resolve((string) $request['query']),
    'locate' => fn (array $request) => $store->locate((string) $request['file'], (int) $request['line']),
    'callers' => fn (array $request) => $store->callers((string) $request['symbol']),
], $store);
