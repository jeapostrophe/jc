# Performance Plan

All items completed.

## Phase 1: Main-thread blockers

- [x] Move `generate_diff_inner()` to background thread via flume channel (`diff_view.rs`, `workspace/mod.rs`)
- [x] Cap terminal PTY buffer coalescing at 64KB (`view.rs`)

## Phase 2: Unnecessary re-renders

- [x] Guard `cx.notify()` in bell subscription — only notify on new insertion (`workspace/mod.rs`)
- [x] Terminal mouse handlers — already guarded after inspection; no changes needed

## Phase 3: Terminal render optimization

- [x] **3A: Dirty tracking** — `content_generation` counter skips bg+text passes when content unchanged (`view.rs`, `render.rs`)
- [x] **3B: Row-based text shaping** — one `shape_line()` per row (~25 calls) instead of per cell (~2000) (`render.rs`)

## Phase 4: Minor wins

- [x] Replace `pbpaste` subprocess with `arboard` crate (`workspace/mod.rs`, `Cargo.toml`)
- [x] Cap file watcher drain loop at 100 iterations (`file_watcher.rs`)
- [x] Pre-allocate diff parse string with capacity heuristic (`diff_view.rs`)
- [x] Picker uses `v_virtual_list` instead of `.children()` (`picker.rs`)
