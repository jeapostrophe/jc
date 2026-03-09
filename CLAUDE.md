* Don't read the TODO.md file.
* If you see any `[?]` labels in the README.md task checklist, triage them first: read the task, examine the relevant code, and replace `[?]` with the correct difficulty label (`[T]`/`[E]`/`[H]`/`[D]`) before starting other work.
* When updating gpui-component, run `scripts/update-gpui-component.sh` to re-vendor from cargo cache and apply patches.
* When a picker confirm handler calls a method that sets focus itself (e.g. `switch_to_session`), drop `pre_picker_focus` instead of restoring it — the stale handle points at a view that may no longer be in a pane, causing focus to be lost.
