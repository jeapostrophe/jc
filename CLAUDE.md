* Don't read the TODO.md file.
* Use 'cargo clippy' to check for stylistic errors.
* Use 'cargo fmt' to format the entire project or 'rustfmt' to format individual files.
* Prefer using generics and type parameters, rather than 'dyn'.
* Prefer ::default() rather than ::new().
* Prefer #[derive(Default)] to manual implementation.
* Check for README.md documentation files in subdirectories relevant to the code you're modifying.
* Write and maintain examples.
* Check for CLAUDE.md files in subdirectories relevant to the code you're modifying.
* If you see any `[?]` labels in the README.md task checklist, triage them first: read the task, examine the relevant code, and replace `[?]` with the correct difficulty label (`[T]`/`[E]`/`[H]`/`[D]`) before starting other work.
* When updating gpui-component, run `scripts/update-gpui-component.sh` to re-vendor from cargo cache and apply patches.
* When a picker confirm handler calls a method that sets focus itself (e.g. `switch_to_session`), drop `pre_picker_focus` instead of restoring it — the stale handle points at a view that may no longer be in a pane, causing focus to be lost.
