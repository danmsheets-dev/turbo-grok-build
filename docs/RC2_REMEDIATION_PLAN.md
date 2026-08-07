# RC2 Remediation Plan — Windows crash, line endings, and 42 red tests

**Date:** 2026-08-06 · **Branch:** `dev` @ `a02d0dc9e` (RC2 / `0.2.119-r2`)

> **STATUS: COMPLETE.** Every step below was executed and verified. Final state: `cargo test --workspace --lib
> --no-fail-fast` = **26 652 passed / 0 failed**; `cargo check --workspace --all-targets` clean; the cpal crash
> repro exits 0; `scripts/check-line-endings.sh` passes with 0 CRLF blobs in the index and 0 BOMs. The workspace
> went from **235 failures to 0** — and from a suite that could not finish at all, because the crash in §1 aborted
> the harness partway. Kept as the design record; see `CHANGELOG.md` for the shipped summary.
>
> Two notes on what execution changed versus this plan: step 1 took the **dedicated audio-thread** route, not the
> vendored cpal fork — cpal `master` (0.18.x) still has the identical `OnceLock` bug, so a version bump was never
> a fix and a fork would have been a permanent maintenance liability. And the sweep in step 5 revealed far more
> than the 42 known-red tests: `xai-grok-shell` alone had 457, of which 404 were a single `/tmp` literal.

**Method:** 5 parallel investigators + 2 adversarial verifiers + synthesis. The crash and the line-ending plan were
independently re-verified; the crash reproduction was additionally re-run by hand (segfault, exit 139) before this
document was written.

---

## The short version

Two things are broken **for users**. Everything else is test debt.

1. **RELEASE BLOCKER — Windows voice capture hard-crashes the app on the second dictation.** Not a test artifact:
   reproduced against the real `xai-grok-voice` crate with the exact feature set `turbo.exe` compiles, and the
   shipped binary carries the code path. Voice mode is **on by default**. No panic, no unwind, no message — the
   process dies at exit 139 and takes any unsent draft and session state with it.
2. **The shipped system prompt contains 465 stray CR bytes**, and the test that guards it is green *only* on
   Windows — it fails on every Linux/macOS/CI checkout.

Not broken for users, but worth knowing: **34 files carry CRLF in the git index** (my earlier "the index is clean"
claim was wrong — `git ls-files --eol` is authoritative and reports 3334 `i/lf`, 34 `i/crlf`, 9 `i/-text`), 3313
files have on-disk bytes differing from committed bytes while `git status` reports clean, and 13 files carry a
UTF-8 BOM. All 34 CRLF blobs entered in a single commit, `1c1d263d4` (RC14).

**None of the 42 red tests is catching a product bug.** They are Windows-host assumptions and stale goldens.

---

## 1. The release blocker, in detail

`cpal` 0.15.3 caches a WASAPI `IMMDeviceEnumerator` in a process-global `OnceLock`
(`cpal-0.15.3/src/host/wasapi/device.rs:836-861`). `com_initialized()` is called *only inside* `get_or_init`, so the
enumerator is created in the COM apartment of whichever thread touched cpal first. When that thread exits, its
`thread_local` `ComInitialized::Drop` runs `CoUninitialize`, tearing down the apartment and unmapping
`MMDevAPI.dll` — while the static keeps the now-dangling interface pointer. Every later cpal call from any other
thread dereferences the freed vtable.

Debugger evidence from the verifier: `EXCEPTION_ACCESS_VIOLATION`, read of `0x00007FFB00F012E0`
(MMDevAPI base + 0x612E0), memory state `MEM_FREE` / `PAGE_NOACCESS`, MMDevAPI not loaded at fault time — identical
in the test harness and in a plain non-libtest binary linking the real crate.

**The user path:** voice mode defaults to `true` (`resolve_voice_mode_enabled`,
`crates/codegen/xai-grok-pager/src/app/mod.rs:223-235`). Every push-to-talk hold calls `spawn_pcm_capture`
(`crates/codegen/xai-grok-voice/src/audio/capture.rs:66-120`), which spawns a dedicated thread that calls
`default_input_device()` and then exits at stop. **Second hold = hard crash.** Same for Codex Live
(`live/session.rs:184`) and for `/doctor` after a dictation (`app/dispatch/prompt.rs:73`).

It does **not** require audio hardware — the enumerator is constructed and cached even when `default_input_device()`
returns `None`, which is why a mic-less machine faults too. Windows-only: macOS routes cpal through the
`__mic-capture` subprocess and Linux shells out to `pw-record`/`parec`/`arecord` (`audio/mod.rs:36-52`).

**Why `turbo doctor` never crashes today:** it probes on the main thread (`pager-bin/src/main.rs:1887`), which wins
the race. That is luck, not safety.

**Interim mitigation for users, until fixed:** set `GROK_VOICE_MODE=0` (or `[voice] enabled = false` in config) to
disable voice mode, or simply never dictate twice in one session.

---

## 2. Ordered plan

Effort: **~5-8 engineer-days**, plus one quiet-tree window for the line-ending sequence.
Steps 1-5 and 6-12 are two independent tracks that can run in parallel (verified: none of the test-fixture files is
among the 34 index-CRLF files).

### Track A — correctness

| # | Step | Effort | Kind |
|---|------|--------|------|
| 1 | **Fix the cpal WASAPI use-after-free.** *First check whether cpal >0.15.3 already fixes it* — that decides everything. If not, add a `[patch.crates-io]` fork of 0.15.3 (the mechanism is already proven in this repo for `async-openai`): call `com_initialized()` on **every** `get_enumerator()` call, not just inside `get_or_init`, and drop the `OnceLock` so `CoCreateInstance` runs per call (sub-ms, cold path). **Do not** convert the static to a plain `thread_local!` — TLS destructor ordering against cpal's own `COM_INITIALIZED` is unspecified. | medium | **product** |
| 2 | **Split live host probes out of `collect_report_with`.** Extract the pure snapshot→report step so the three doctor fixture tests stop touching audio hardware. Defense-in-depth for step 1, not a substitute. | small | test |
| 3 | **Make the doctor goldens hermetic** — pin an explicit 5-theme fixture instead of importing `ThemeKind::ALL.len()`, and add a *structural* test asserting the real property. | trivial | test |
| 4 | **Platform-gate 13 ssh-wrap + 17 bash-Tab tests** to match the product's own `cfg!(not(windows))` guards. **Blocked on knowing whether CI has a unix runner** (see Unknowns). | small | test |
| 5 | **Fix 4 Windows-hostile fixtures** — `/tmp` literal, POSIX `file://` URL, path-separator spelling, and one superseded keybinding contract. | small | test |

### Track B — line endings (strict sequence)

| # | Step | Effort | Kind |
|---|------|--------|------|
| 6 | **GATE: publish the actual `.gitattributes` text for review before running anything.** The catch-all must be `* text=auto`, never bare `* text` — see Safety below. | trivial | — |
| 7 | **Decide the prompt-template policy.** This is a *shipped-asset* decision, not whitespace. Recommended: normalize to LF and regenerate `prompt_encrypted.rs`, which fixes the latent non-Windows test failure. Must be its **own commit**. | small | **product** |
| 8 | Commit 1: `.gitattributes` alone, no content change. | trivial | — |
| 9 | Commit 2: `git add --renormalize .` — **34 files, +25667/-25667**. Review surface is 34 files; 3313 more change on disk with zero diff. | small | — |
| 10 | Commit 3: `.git-blame-ignore-revs`. Commit 4: strip 13 UTF-8 BOMs (a second corruption axis that `eol=lf` cannot touch). | trivial | — |
| 11 | **Recurrence guards:** CI check `git ls-files --eol \| grep -E '^i/(crlf\|mixed)' && exit 1`, a BOM scan, a shipped-script LF assertion, and `.editorconfig`. Must land *after* step 9. | trivial | — |
| 12 | Delete the now-dead `release.yml:66-73` CRLF workaround; switch any worktree-copying packaging path to `git archive`. | small | **product** |

---

## 3. Safety findings that changed the plan

The line-ending plan was judged **UNSAFE as first written**. Two real hazards were caught before anything ran:

- **A bare `* text` or `* text eol=lf` rule would silently corrupt ~2.3 MB across 9 binary files** — including
  `assets/game_mode/office_bg.png` (5817 lone CR bytes) and `Roboto-Regular.ttf`, which is `include_bytes!`-embedded
  as `BUNDLED_FONT` and would ship corrupted. `git check-attr` proves the distinction: `text=auto` → `text: auto`
  (git's NUL heuristic still protects binaries); `text` → `text: set` = forced conversion, no detection. Use
  `text=auto` plus explicit `*.png binary` / `*.ttf binary` / `*.wasm binary` belt-and-braces rules.
- **Normalizing `templates/*.md` breaks the encrypted-prompt test *and* changes a shipped asset.** The XOR key is
  position-dependent, so removing 465 CR bytes shifts every subsequent byte — there is no partial-damage mode. This
  is exactly the "silently changes shipped assets" failure the review was asked to hunt for.

## 4. Explicitly do NOT do these

- **Do not** add `tests/fixtures/** -text` to "freeze" the 100 mixed fixture files. `-text` stores worktree bytes
  verbatim, which would **commit the corruption permanently into history** (measured: 135 files / 25768 lines with
  that rule, versus 34 files without it).
- **Do not** re-pin the doctor golden from 5 to 19. These are golden tests of the *formatter*, which never touches
  `ThemeKind::ALL`. Importing the live catalog size guarantees the same red the next time anyone adds a theme.
- **Do not** treat the main-thread cpal warm-up as the fix for step 1. It makes the repro pass and it is why
  `turbo doctor` survives today, but it only reorders who wins the race — the apartment-crossing UB remains.
- **Do not** make `plan_ssh_wrap` succeed on Windows to turn 13 tests green. The `cfg!(windows)` guard is deliberate
  and has its own test asserting it.
- **Do not** regenerate `prompt_encrypted.rs` in the same commit as the renormalization — it destroys the
  `git show -w --ignore-cr-at-eol` = empty review protocol, which is the only tractable way to review 25667 lines.
- **Do not** land the EOL CI guard before the renormalization commit; it will fail immediately on the existing 34.

## 5. Open unknowns — resolve these before starting

1. **Does cpal >0.15.3 already fix the enumerator lifetime?** Nobody checked. Decides fork vs. version bump. Do this
   first — a fork is a maintenance liability taken on unnecessarily if upstream already landed it.
2. **Does the CI matrix include a unix runner?** Load-bearing for step 4. With one, `#[cfg(unix)]` is correct. On a
   Windows-only matrix it silently deletes 30 tests, converting visible red into invisible zero coverage — and
   `#[ignore = "reason"]` is the honest choice instead.
3. **Does normalizing the prompt templates change model behavior?** Almost certainly nil (only line terminators),
   but "almost certainly" is the honest word and nobody has A/B'd it.
4. **Are there more failures hidden behind the crash barrier?** 42 red tests are accounted for; no complete
   `--lib` run has ever finished, so the true count past the abort is unknown until step 1 lands.
5. **What is still writing lone LFs into `.gitignore`?** Its byte pattern proves two different writers appending over
   time — the damage is ongoing, not a one-time accident. Unidentified, so the recurrence guard is CI-only.

## 6. Confidence

The crash diagnosis and the line-ending forensics were each adversarially verified by a second agent, and the crash
was reproduced a third time by hand. The five test diagnoses in steps 3-5 were derived once and **not** independently
verified — treat those as high-confidence-but-unchecked, and expect small surprises during execution.
