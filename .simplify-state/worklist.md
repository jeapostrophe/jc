# Simplify Worklist

Generated: Sat Mar  7 09:07:27 MST 2026
Repository: /Users/jay/Dev/jc
Branch: simplify/systematic-20260307-090727
Permission mode: acceptEdits

## Phase 0: Split Oversized Files

- [hash:000000000000] jc-app/src/views/workspace.rs (2124 lines)

## Phase 1: Individual Files

- [hash:dfda2bd4bd31] jc-app/src/app.rs (222 lines)
- [hash:0d3549df3f30] jc-app/src/file_watcher.rs (59 lines)
- [hash:ecc13c1752f1] jc-app/src/ipc.rs (169 lines)
- [hash:b82b1f43bed9] jc-app/src/language.rs (77 lines)
- [hash:a85f79a60d07] jc-app/src/main.rs (130 lines)
- [hash:5ddddffef8f6] jc-app/src/notify.rs (250 lines)
- [hash:5d74613abd3a] jc-app/src/outline.rs (133 lines)
- [hash:2bd8e60cd113] jc-app/src/views/code_view.rs (238 lines)
- [hash:072ff4743b3f] jc-app/src/views/comment_panel.rs (99 lines)
- [hash:86e76b44a4ce] jc-app/src/views/diff_view.rs (368 lines)
- [hash:87a17f105553] jc-app/src/views/keybinding_help.rs (169 lines)
- [hash:741e776cee75] jc-app/src/views/mod.rs (71 lines)
- [hash:133bb5ee2fe7] jc-app/src/views/pane.rs (94 lines)
- [hash:5325baac132a] jc-app/src/views/picker.rs (1401 lines)
- [hash:5c6f863cadbf] jc-app/src/views/project_state.rs (158 lines)
- [hash:4ec1b4b99a92] jc-app/src/views/project_view.rs (81 lines)
- [hash:5f3da758c36c] jc-app/src/views/reply_view.rs (267 lines)
- [hash:d0e71bb270f8] jc-app/src/views/session_state.rs (115 lines)
- [hash:0884bc2db81f] jc-app/src/views/todo_view.rs (308 lines)
- [hash:3e75fd915cb1] jc-core/src/config.rs (110 lines)
- [hash:47eb2dfdfdbf] jc-core/src/hooks.rs (138 lines)
- [hash:3ad4bb194ff5] jc-core/src/hooks_settings.rs (127 lines)
- [hash:0def0e885420] jc-core/src/lib.rs (10 lines)
- [hash:c2a7c69eeeab] jc-core/src/model.rs (41 lines)
- [hash:513b89761d9f] jc-core/src/problem.rs (129 lines)
- [hash:6e9718730439] jc-core/src/session.rs (483 lines)
- [hash:ab43468b2b9c] jc-core/src/snippets.rs (152 lines)
- [hash:cc0412ae0cbe] jc-core/src/status_script.rs (124 lines)
- [hash:fcef3b94c406] jc-core/src/theme.rs (122 lines)
- [hash:7d8df547315c] jc-core/src/todo.rs (794 lines)
- [hash:9fc3befbd21e] jc-terminal/src/colors.rs (112 lines)
- [hash:79d570756e16] jc-terminal/src/input.rs (78 lines)
- [hash:13432ae2cf09] jc-terminal/src/lib.rs (9 lines)
- [hash:1678b06509bd] jc-terminal/src/pty.rs (84 lines)
- [hash:8ef2a343a0f9] jc-terminal/src/render.rs (226 lines)
- [hash:de39bdee8c7d] jc-terminal/src/terminal.rs (92 lines)
- [hash:97b6996a0de3] jc-terminal/src/view.rs (548 lines)
- [hash:e629b16dad8c] make.sh (10 lines)
- [hash:dc2e78afa3fe] scripts/bundle.sh (53 lines)
- [hash:e743595f9be8] scripts/update-gpui-component.sh (32 lines)
- [hash:e9bd98f92325] scripts/update-outline-queries.sh (13 lines)
- [hash:c574485ba153] status.sh (2 lines)

- [hash:171b3255eada] jc-app/src/views/workspace/mod.rs (1209 lines) [from phase 0 split]
- [hash:98a4fff5022b] jc-app/src/views/workspace/pickers.rs (483 lines) [from phase 0 split]
- [hash:1f107f18e778] jc-app/src/views/workspace/problems.rs (208 lines) [from phase 0 split]
- [hash:679f4ce74232] jc-app/src/views/workspace/render.rs (280 lines) [from phase 0 split]

## Phase 2: Leaf Directories

- [hash:6adcbec40985] jc-app/src/views/ (13 files)
- [hash:a6511466bbed] jc-terminal/src/ (7 files)
- [hash:329fb11eb69a] jc-core/src/ (11 files)
- [hash:87a112572e43] jc-app/src/ (7 files)
- [hash:8fa2edd1c413] scripts/ (3 files)

## Phase 3: Parent Directories (bottom-up)


## Phase 4: Cross-Cutting Review

- [hash:f62fa5fb8005] whole-repo (cross-cutting patterns, shared abstractions, API consistency)

