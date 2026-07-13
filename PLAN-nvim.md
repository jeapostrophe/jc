# PLAN-nvim → moved

The plan to reimagine `jc` as an nvim plugin now lives in its own repo:

**`~/Dev/jc.nvim`** — see `~/Dev/jc.nvim/PLAN.md`.

Rationale and the decisions that shaped it:
- The used slice of `jc` (fixed [Claude | TODO | Terminal] layout, session
  switching, jump-to-WAIT, send-to-Claude) is reimplemented as ~1–1.5k lines of
  Lua; Neovim supplies the terminal, editor, treesitter, and pickers.
- UUIDs are **assigned** via `claude --session-id <uuid>` (verified), not detected
  via hooks — dissolving jc's flaky new-session / multi-session-per-cwd / `/clear`
  handling.
- The only ambient signal kept is "Claude isn't working", surfaced in the session
  picker via minimal `$NVIM`-guarded RPC hooks (no HTTP server).

This file is just a breadcrumb; do not duplicate the plan here.

## Port: worktree transcript buckets (from the Rust jc fix, 2026-07-13)

jc.nvim has the same bug the Rust jc just fixed. `lua/jc/claude.lua`'s
`transcript_path(home, abspath, uuid)` encodes only `abspath` (the project
realpath), so it looks solely in the project's **root** bucket
`~/.claude/projects/<encode(root)>/`. But Claude keys the bucket off the launch
cwd, so a session run in a git worktree at `<root>/.claude/worktrees/<branch>`
lands in a **sibling** bucket `<encode(root)>--claude-worktrees-<branch>`. jc.nvim
therefore misses it:

- `session_expired` (`lua/jc/session.lua`) → `file_exists(transcript_path(...))`
  returns false for a live worktree session → it's flagged expired, and `switch`
  revives it with a fresh uuid (losing the real conversation) / `start` skips it.

Fix (mirror `jc_core::claude`, prefix-scan only — no `git worktree list`):
- [ ] In `lua/jc/claude.lua`, add `M.session_dirs(home, abspath)` returning the
      root bucket plus any sibling dirs under `~/.claude/projects/` whose name
      starts with `encode_dir(abspath) .. "--claude-worktrees-"` (the encoded
      `/.claude/worktrees/`). The full `--claude-worktrees-` separator prevents a
      sibling project (`pgm` vs `pgm-2`) from matching. Scan with
      `vim.loop.fs_scandir`.
- [ ] Add `M.transcript_exists(home, abspath, uuid)` = any bucket has
      `<uuid>.jsonl`; keep `transcript_path` for the still-root-only launch/resume
      decision, or point it at the newest bucket if resume must attach worktree
      history.
- [ ] Rewire `session_expired` (and any other `file_exists(transcript_path(...))`
      liveness check) through `transcript_exists`.
- [ ] Tests in `tests/claude_spec.lua`: encode maps `/` and `.` to `-`; the
      `--claude-worktrees-` prefix predicate rejects the `pgm-2` decoy; a uuid whose
      jsonl lives only in a worktree bucket is found.
- [ ] Out-of-tree worktrees (`git worktree add ../elsewhere`) are not covered by
      the prefix scan — same known gap as the Rust side; note it, don't shell out
      to git unless it proves necessary.
