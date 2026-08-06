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

## Security reports

Please report security issues through the process described in
[`SECURITY.md`](SECURITY.md). Do not open a public issue for vulnerabilities.

## Licensing of this source

By downloading or using this source, you agree that your use is governed by
the Apache License, Version 2.0. No contributor license agreement is offered
because external contributions are not accepted.
