* Don't read the TODO.md file.
* If you see any `[?]` labels in PLAN.md, triage them first: read the task, examine the relevant code, and replace `[?]` with the correct difficulty label (`[T]`/`[E]`/`[H]`/`[D]`) before starting other work.
* When updating gpui-component, run `scripts/update-gpui-component.sh` to re-vendor from cargo cache and apply patches.
* When a picker confirm handler calls a method that sets focus itself (e.g. `switch_to_session`), drop `pre_picker_focus` instead of restoring it — the stale handle points at a view that may no longer be in a pane, causing focus to be lost.
* gpui performance rules:
  - Never use `window.refresh()` — use `cx.notify()` on the specific entity.
  - Only call `cx.notify()` when state actually changed. Guard with a comparison or use the return value of mutations (e.g. `HashSet::insert()` returns `false` if already present).
  - Never call `cx.notify()` inside `cx.observe()` callbacks unless unavoidable — it cascades re-renders.
  - Always store `Subscription` return values from `cx.observe()` / `cx.subscribe()` in a struct field. A dropped `Subscription` leaks observer callbacks.
  - Use `uniform_list` for rendering collections that can exceed ~50 items. Never `.children()` an unbounded `Vec`.
  - Minimize nested background layers — gpui has 5-6x pixel overdraw from alpha-blended rounded rects.
  - Never do blocking I/O (git, filesystem, subprocess) on the main thread or in async tasks on the main executor. Use `cx.background_executor().spawn()` or `std::thread::spawn`.
