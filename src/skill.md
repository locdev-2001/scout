---
name: scout
description: Find what a change affects - every caller of a function, method or class, walked back to its entry points (routes, commands, jobs, events), from the code on disk now. Use before changing a signature, renaming, deleting or changing the behavior of a function; when reading a stack trace; and whenever the question is "who calls X" or "what does changing X affect". Not a text search - keep grep/rg for text, definitions, config and logs.
allowed-tools: Bash(scout callers:*), Bash(scout locate:*)
---

# scout: who calls this code

A text search finds lines that spell a name. `scout` finds the calls that reach the code: it follows types, imports,
inheritance and framework wiring (Laravel routes, events, jobs, facades, scopes; Vue/React components; Rust traits),
and lists what it cannot type instead of dropping it.

## Ask

```bash
scout callers 'App\Services\OrderService::save' --compact   # a method
scout callers saveOrder --compact                            # a name (every symbol it names)
scout callers app/Services/OrderService.php:120 --compact    # the function at a line
scout locate app/Services/OrderService.php:120               # which function a stack-trace line is in
```

- `--compact` (use it): the direct callers with their code, then every other affected symbol grouped by file, then
  the unresolved lines by file.
- `--depth 1`: direct callers only, when the whole tree is too large to read.
- `--tests`: also list callers inside tests (otherwise only counted).
- `--repo DIR`: another repository than the current directory's.

## Read the answer

- `exact`: proven call. `likely`: through a parent type or framework magic; check it.
- `unresolved`: a call the tool could not type (a receiver of unknown type). Read each one: it may be a real caller.
- `Affected entry points`: the routes, commands, jobs and events that reach the code, the places to test after a
  change.
- The first question in a repository may take some seconds while its code is read; later ones take milliseconds.

## While you change code

The hooks add the direct callers of each function you change, whatever changed it (an edit, a written file, a
script), and at the end of a turn they list the changed functions whose callers you have not seen yet. Check that
each caller still fits the change before finishing.

## When not to use it

scout does not replace grep. Finding where something is defined, a message, a config key, a CSS class or a log line
is a job for grep/rg.
