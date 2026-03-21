# Performance Plan

## Phase 1: Main-thread blockers (biggest impact)

- [x] [H] Move `generate_diff_inner()` off main thread
  - `jc-app/src/views/diff_view.rs:346-359`
  - Currently does blocking git2 calls (open repo, walk tree, diff) inside the async poll task
  - `include_untracked(true)` + `recurse_untracked_dirs(true)` forces full directory walk — 100-500ms on large repos
  - Move to `cx.background_executor().spawn()` and store result via channel or shared state
  - The 2-second poll in `workspace/mod.rs:248-286` should kick off the background job and pick up results next tick

- [x] [E] Cap terminal buffer coalescing
  - `jc-terminal/src/view.rs:165-202`
  - `while try_recv()` loop accumulates unbounded PTY data before `advance()` + `cx.notify()`
  - Cap coalesced buffer (e.g. 64KB), process in chunks, yield to event loop between chunks

## Phase 2: Unnecessary re-renders

- [x] [E] Guard `cx.notify()` in terminal mouse handlers
  - `jc-terminal/src/view.rs` — mouse_move was already guarded; others genuinely change state
  - No changes needed after inspection

- [x] [E] Guard `cx.notify()` in bell subscription
  - `jc-app/src/views/workspace/mod.rs:458`
  - `pending_events.insert()` returns bool — only notify if it returned `true` (new insertion)

- [ ] [H] Avoid full terminal repaint on cursor blink
  - `jc-terminal/src/view.rs:206-231`
  - Every 500ms, cursor toggle triggers full 3-pass cell repaint via `cx.notify()`
  - Options: separate cursor overlay, or track content-dirty vs cursor-dirty and skip cell repaint

## Phase 3: Render cost reduction

- [ ] [H] Optimize terminal render passes
  - `jc-terminal/src/render.rs:86-120`
  - Three full passes over every cell: backgrounds, selections, text
  - `shape_line()` per cell is expensive (text layout)
  - Options: merge passes into single iteration, batch `shape_line()` calls, cache shaped text between frames when content unchanged, use dirty-region tracking

- [x] [E] Use `uniform_list` in picker
  - `jc-app/src/views/picker.rs:278-295`
  - Currently renders up to 200 items via `.children(results)` — all laid out even if off-screen
  - Replace with `uniform_list` for O(1) layout

## Phase 4: Minor wins

- [x] [E] Replace `pbpaste` subprocess with native clipboard API
  - `jc-app/src/views/workspace/mod.rs:82-88, 1526-1536`
  - Spawns `pbpaste` 15 times at 200ms intervals; each is a blocking `Command::new().output()`
  - Use gpui's clipboard API or direct `NSPasteboard` objc call

- [x] [T] Tighten file watcher drain loop
  - `jc-app/src/file_watcher.rs:50-54`
  - `while try_recv().is_ok() {}` tight-loops; add a small cap or yield

- [x] [T] Reduce diff string allocations
  - `jc-app/src/views/diff_view.rs:361-395`
  - Line-by-line `push_str` into `current_content` causes repeated reallocations
  - Pre-allocate with capacity estimate or use a rope/rope-like structure
