# Stopping a Hyper run on Windows

## The failure you may have seen

```
ERROR: The process with PID 33740 (child process of PID 27808) could not be terminated.
Reason: The operation attempted is not supported.
```

That message means a harness (or Task Manager / `taskkill`) tried to kill a
**single PID** that is part of a larger process tree. On Windows, some children
(console hosts, job-protected workers, broken intermediate parents) refuse a
direct terminate. Killing the guessed PID leaves orphans.

## Do this instead: kill the Job Object

Hyper can run inside a Win32 Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
Closing the last handle to that job terminates **every** process in the tree
(agent, terminal children, MCP servers, `rg`, engines, …).

### Opt-in (harness / CI)

```text
hyper --job-object -p "…"
# or
set HYPER_JOB_OBJECT=1
hyper -p "…"
```

Interactive users leave this **off**. Nested jobs or an already-jobbed parent
can make assignment fail; Hyper logs a warning and continues.

### What the harness should do

1. Launch Hyper with `--job-object` / `HYPER_JOB_OBJECT=1`.
2. Prefer holding a job handle if you created the job yourself and assigned
   Hyper into it; **close that handle** to stop the run.
3. If you only have Hyper's PID: open the process, query its job
   (`IsProcessInJob` / `QueryInformationJobObject`), and terminate the **job**,
   not a grandchild PID you scraped from logs.
4. Do **not** rely on `taskkill /T` against a random child PID — that is what
   produces the error text above.

### Internal process groups

Even without `--job-object`, Hyper enrolls terminal/MCP/LSP children into
per-scope Job Objects (`ProcessScope` / `ProcessGroup`) and reaps them on
session exit via `kill_all()`. Short-lived tools like `grep` (ripgrep) inherit
the parent job when `--job-object` is set; they are not enrolled individually
because their lifetime is sub-second.

Deliberately **not** job-enrolled:

- The auto-updater child that **replaces** Hyper (must outlive the parent).
- Interactive TTY children that must share the console session when the user
  is not using a harness.

## Worktree cleanup and long paths

Deep caches (`.godot/imported/…`, nested `node_modules`) can make
`remove_dir_all` fail with `Filename too long`. Hyper uses `\\?\` long-path
helpers (`xai_grok_paths::windows_long_path` / `remove_dir_all_long`) for
worktree removal and sets `core.longpaths=true` on worktrees it creates.
