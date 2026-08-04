# Land patch: wire `turbo tree` in pager-bin

**Status: APPLIED** (RC13 source land).

- `cli.rs` → `Command::Tree(TreeArgs)`
- `pager-bin/main.rs` → dispatch `tree_cmd::run`
- Root `Cargo.toml` already lists `crates/codegen/xai-workspace-tree`
- Nested `[workspace]` removed from `xai-workspace-tree/Cargo.toml`

This file can be deleted after ship if desired.
