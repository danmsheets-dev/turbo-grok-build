# Contributing

This repository does **not** accept external pull requests or unsolicited
patches.

SpaceXAI develops this software internally. The public tree is published for
source transparency and local builds under the terms of the Apache License,
Version 2.0 (see [`LICENSE`](LICENSE)).

## Working in a local checkout

Run this once per clone so `git blame` skips the whitespace-only line-ending
normalization and keeps attributing each line to the commit that wrote it:

```sh
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

Web hosts read [`.git-blame-ignore-revs`](.git-blame-ignore-revs) on their own;
only the local CLI needs opting in.

## Line endings

The index is LF everywhere. [`.gitattributes`](.gitattributes) enforces it, and
[`.editorconfig`](.editorconfig) stops editors from writing the drift in the
first place. Files that ship to end users or are compiled into the binary with
`include_str!` are pinned `eol=lf` explicitly, so a Windows build and a Linux
build of the same commit produce the same bytes.

**`git status` cannot show a line-ending problem.** Git's conversion is
deliberately asymmetric — checkout will not add CR to a blob that already has
some, and check-in will not strip CR from a blob that has none — so a worktree
holding either spelling cleans back to the same blob and the tree reports
clean. The diagnostic is:

```sh
git ls-files --eol          # i/ is the stored blob, w/ is your worktree
git ls-files --eol | grep -E '^i/(crlf|mixed)'    # must print nothing
```

Run the full set of guards — index line endings, UTF-8 BOMs, and CR bytes
inside LF-pinned paths — the same way CI does:

```sh
./scripts/check-line-endings.sh
```

If the index ever does drift, `git add --renormalize .` is the fix; note that
it updates the index but **not** the worktree, so re-checkout afterwards.

## Security reports

Please report security issues through the process described in
[`SECURITY.md`](SECURITY.md). Do not open a public issue for vulnerabilities.

## Licensing of this source

By downloading or using this source, you agree that your use is governed by
the Apache License, Version 2.0. No contributor license agreement is offered
because external contributions are not accepted.
