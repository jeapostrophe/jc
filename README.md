<p align="center">
  <img src="icon.png" width="128" height="128" alt="jc icon" />
</p>

<h1 align="center">jc</h1>

<p align="center">
  Orchestrate multiple Claude Code sessions across projects.<br>
  Draft, send, and switch — all from one window.
</p>

<p align="center">
  <a href="https://github.com/jeapostrophe/jc/actions/workflows/ci.yml"><img src="https://github.com/jeapostrophe/jc/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/platform-macOS-blue" alt="macOS" />
  <img src="https://img.shields.io/badge/built_with-Rust_%2B_GPUI-orange" alt="Rust + GPUI" />
</p>

<p align="center">
  <a href="#why">Why</a> · <a href="#getting-started">Getting Started</a> · <a href="#keybindings">Keybindings</a> · <a href="DESIGN.md">Design</a> · <a href="ARCH.md">Architecture</a>
</p>

![jc screenshot — 3-pane layout with Claude terminal, TODO editor, and code viewer](screenshot.png)

## Why

Claude takes minutes per task. If you wait, you get four cycles an hour. If you switch to another session while Claude works, you get twelve — but only if you can come back without losing your place.

jc keeps your place outside your head. Each session's notes live in the project's TODO.md, under a `### WAIT` marker: you type the next instruction there whenever you think of it, and Cmd-Enter sends it and files it into the message log. Coming back to a session means reading what you already wrote, not reconstructing it.

Switching is cheap. Cmd-P lists every session in every project; each session keeps its own pane layout, cursor position, and terminal scrollback, restored on switch-back. A red `*` marks the sessions whose Claude terminal has printed something since you last had it on screen, so the picker tells you where the work moved while you were elsewhere. Desktop notifications stay out of the way — only a blocked session (permission prompt, API error) interrupts you, and only when jc isn't the front app.

See [DESIGN.md](DESIGN.md) for the full rationale.

## Getting Started

```bash
# Build and run as macOS .app bundle
./make.sh

# Or run directly via cargo
cargo run -p jc-app

# Register a project directory
cargo run -p jc-app -- .
```

Config and state live in `~/.config/jc/` (`config.toml`, `state.toml`, `theme.toml`), or `$JC_CONFIG_DIR` when it is set. `./make.sh` also builds `jcc.app`, jc as Cira's client: see [jcc](#jcc-jc-as-ciras-client).

## Core Concepts

### Projects and Sessions

A **project** is a code repository registered with jc. Each project has one or more **sessions** — ongoing Claude Code conversations. Sessions are defined in the project's TODO.md:

```markdown
# Claude
## Refactor auth module
> uuid=abc123-def456-...
### Message 0
first instruction sent to claude
### Message 1
second instruction
### WAIT
Notes for next message go here.
```

The `### WAIT` marker separates what you've sent from what you're drafting. Everything below WAIT is draft text — jc never treats it as log, even if it contains a quoted `### Message N` heading. When you send (Cmd-Enter), the draft becomes a numbered message and WAIT moves below it — so you have the recent history of what you asked.

The log is bounded: each send keeps the 25 most recent messages of that session and drops the rest, so a long-running session settles into a sliding window (`### Message 76` through `### Message 100`). jc applies the same bound to every session when it starts, so sessions you have not touched in a while get collected too; the first time it shortens a file it leaves a `TODO.md.bak` beside it.

One exception: truncation stops at a message still waiting on a `@jc(...)` scheduled send, so it can never cancel one. That session's log stays as long as it needs to until the send fires. Numbers are never reused, so a message keeps the number it was sent under. jc never sends TODO.md to Claude — messages are delivered to the session terminal and Claude resumes from its own transcript — so dropping old entries costs it no context.

jc mints the session UUID itself. A new session launches `claude --session-id <uuid>` under a freshly generated v4 UUID that is written to TODO.md at the same moment; existing sessions are resumed on startup with `claude --resume <uuid>`. If Claude has since garbage-collected a session's transcript, jc relaunches it with `--session-id` under the same UUID instead — you lose the conversation, but the heading, its message log, and its WAIT notes stay exactly where they were. `/clear` is handled transparently — the UUID in TODO.md is rewritten in place, with no terminal relaunch.

Per-session metadata lives on `> ` lines under the heading. `> uuid=` and `> last=` are managed by jc; add `> dangerous` by hand to spawn that session's `claude` with `--dangerously-skip-permissions` (takes effect on next spawn). See `ARCH.md` for the full list.

The `> uuid=` line, not the heading text, is what identifies a session — everything jc writes into TODO.md is addressed by it. Rename a heading to whatever you like, and give two headings the same name if that is what you want; sends and scheduled deliveries still land on the right one.

### Session Activity

The Cmd-P session picker marks each session with a single character:

| Marker | Meaning |
|---|---|
| red `*` | This session's Claude terminal has printed output since you last had it on screen |
| green `>` | The session you're on now |
| yellow `~` | In TODO.md but not running — pick it to adopt |
| grey `~` | Disabled (`[D]`) |
| blue `+` | A registered project with no sessions yet |

Sessions sort activity-first, then by recency; the session you're on sorts last, since you're already there. The marker clears when you switch away from a session, so it always means "since you last had it on screen", and the session you're currently on never shows one.

## jcc: jc as Cira's client

**jcc** is a second app built from this same code: jc as the native client of **Cira** (`~/Dev/cira`, the mailbox between you and your sessions). It runs beside jc, so you can move projects to it one at a time while jc keeps running yours. One binary decides which app it is from its file name: run as `jcc` (the `jcc.app` bundle's executable, or `make.sh` through a link named `jcc`) it is jcc; under any other name it is jc. `JC_FLAVOR=jc` or `JC_FLAVOR=jcc` overrides the name (for `cargo run`, whose program is `jc-app`). `./make.sh` (`scripts/bundle.sh`) builds both `jc.app` and `jcc.app`.

**What changes in jcc.** jcc keeps its own TODO for each project, `todo.md` in Cira's store beside the project's scratch pad (`$CIRA_HOME/projects/<project>/todo.md`, normally `~/.cira/projects/<project>/todo.md`), never the project's TODO.md. It opens once Cira names the project (jcc registers the directory with `cira project add` if no Cira project holds it yet; a refusal shows in the TODO pane) and is made at the first save. Two jc projects in one Cira project (a worktree inside a repository) would share one todo.md: only the first opens it, and the other's TODO pane says where to work. Each session is owned by Cira: Cira starts it as a Claude Code **background session**, keeps it in the mode you chose, and stops and removes it; each Claude pane is `claude attach` to the job Cira names, and jcc asks Cira for everything else with `cira session new|mode|stop|rm|label`. Quitting jcc only detaches: sessions keep running, and the next start attaches every heading's session again. While Cira is unreachable one banner says why (`cira` not installed, its daemon not running or not answering) and what to do, and no pane attaches anything new.

- A heading's id line is `> cira=<id>` (the id Cira minted, `CIRA_SESSION` in the terminal pane's shell), and that line, not the heading text, addresses it. The mode is Cira's: Cmd-Shift-P → "Make this session dangerous" (or "auto", Claude Code's `--permission-mode=auto`) asks Cira, which forks the session in the new mode once it is idle, keeping its conversation; the Cmd-P picker marks a dangerous session. jcc always names the mode it asks for: a session you start from jcc (New session, an adopted transcript or blank heading) is dangerous; a migrated heading is dangerous only if it had `> dangerous`, else auto. When jcc refuses something (adopting a conversation jc still runs, a new session before its TODO is open), a banner says why. Disabling a session (`[D]`) stops it; renaming the heading to `## [DELETED] …` removes it — each once, when the marker changes from what jcc last saw, even if you edit it while jcc is closed. Renaming the heading renames the session in Cira, and a rename made on Cira's page renames the heading.
- The **General Terminal** is the session's shell, kept by Cira and shared with its page (`s`); it outlives jcc. It is headed "Terminal · not shared" when Cira gives none and jcc runs its own.
- The Cmd-P marker comes from the session's state — from Cira when Cira knows the session, else from jcc's own hooks — never from what the terminal prints, so a ticking "3m 42s" is not activity: bold red `!` a permission dialog is waiting (sorted first, from any project, and shown even on the session you are on), red `*` it finished a turn (or stopped on an API error) since you last had it on screen, grey `~` its turn is over and background agents it started still run, grey `x` the session process has ended. Each row also shows its project's open Cira blocks (`4 blocks`), and the TODO pane's header says the same (`TODO · 4 blocks`), read with `cira status --json` every 2 seconds, off the main thread.

**How jc and jcc stay apart.** Nothing either app acts on is shared:

| | jc | jcc |
|---|---|---|
| TODO | the project's `TODO.md`, `##` headings under `# Claude` with `> uuid=` | `$CIRA_HOME/projects/<project>/todo.md`, `##` headings under `# Cira` (a `# Claude` section there is read too) with `> cira=` |
| Config and state (`state.toml` with its project list, `jc.sock`, jcc's `sessions.toml`) | `~/.config/jc` | `~/.config/jcc` |
| Hooks in a project's `.claude/settings.local.json` | URLs under `/jc-hook/` | URLs under `/jcc-hook/` |
| Bundle id (notifications) | `com.jc.app` | `com.jc.app.jcc` |

jc never reads or writes jcc's file, and jcc never the project's TODO.md. jc also ignores a `> cira=` heading in its file. Each app installs, finds and removes only hooks under its own prefix, under a lock on the project's `.claude` directory, so both can have one project open and quitting either leaves the other's hooks. A project whose hooks cannot be installed (a `settings.local.json` that does not parse, which is left as it is) is named in a banner across the window: its sessions send that app no markers or notifications. `JC_CONFIG_DIR` overrides the config directory of whichever app it is set for; never set one value for both. jc's transcript list (Cmd-Shift-P) leaves out a conversation a background job runs; jcc refuses (with a beep) to continue one that a foreground `claude` still runs, which is how jc runs its sessions.

**Saving.** You, jc and any other editor can all save the same TODO file. Every save first re-reads the file and merges in what someone else saved meanwhile (a three-way merge against what the pane last read); if the two edits conflict, nothing is written to the file: the pane's text goes to `<file>.conflict-<unix time>` beside it, at the conflict and again at every save while it lasts (a send, a schedule), and the banner names that copy. The conflict ends when Cmd-R reloads the file, or when the file goes back to what the pane last read. A save that fails for any other reason says why in the same banner until one works. Saves replace the file whole (a temporary file renamed over it), so no reader ever sees half of one; a symlinked file is written through to its target and keeps its permissions.

**Moving a session from jc to jcc.** Cut its heading block from the project's TODO.md and paste it under `# Cira` in jcc's todo.md (there once jcc has made one session; add it otherwise; a `# Claude` section there works too). jcc moves its conversation to Cira once (`cira session new --resume <uuid>`) and rewrites the line to `> cira=`, but not while a `claude` process still runs that conversation (jc's pane, found by its executable and command line, or Claude Code's session registry): the pane says so until you end it in jc.

## Views

The window has **1, 2, or 3 panes** (Cmd-1/2/3). Any of the five views can go in any pane via Cmd-O.

| View | Description |
|---|---|
| **Claude Terminal** | Claude Code CLI in an embedded terminal. |
| **General Terminal** | Separate shell per session for running tests, inspecting output. |
| **Code Viewer** | Syntax-highlighted source with tree-sitter outline navigation. |
| **TODO Editor** | Markdown editor for session notes. Drafting area below WAIT, message history above. |
| **Global TODO** | View of `~/.claude/TODO.md`. |

Per-session pane layouts are saved and restored on session switch.

## Keybindings

Press **Cmd-?** for the in-app overlay.

### Global

| Key | Action |
|---|---|
| Cmd-1 / 2 / 3 | Set pane layout |
| Cmd-[ / ] | Focus previous / next pane |
| Cmd-O | Open picker (pane views + project files) |
| Cmd-Shift-O | Drill-down picker (symbols / TODO headings) |
| Cmd-P | Session picker (all projects) |
| Cmd-Shift-P | Project actions |
| Cmd-F | Search lines in current editor |
| Cmd-S | Save file |
| Cmd-Enter | Send draft below WAIT to the Claude terminal |
| Cmd-. | Jump to WAIT |
| Cmd-` | Next session (round-robin across all projects) |
| Cmd-Alt-↑/↓ | Scroll other pane by lines |
| Cmd-Alt-PageUp/PageDown | Scroll other pane by pages |
| Cmd-? | Keybinding help |
| Cmd-W / Cmd-M / Cmd-Q | Close window / minimize / quit |

### View-Specific

| Key | Action | View |
|---|---|---|
| Cmd-R | Reload from disk | Code |
| Cmd-C / Cmd-V | Copy / Paste | Terminal |
| Cmd-= / - / 0 | Font size +/-/reset | Terminal |

### Picker

| Key | Action |
|---|---|
| Enter | Confirm |
| Escape / Ctrl-C | Cancel |
| ↓ / Ctrl-N | Next |
| ↑ / Ctrl-P | Previous |
| PageDown / PageUp | Move 10 items |
| Cmd-Shift-Backspace | Toggle session disabled (session picker) |

## Workflow

### Draft → Send → Switch

1. **Cmd-.** jumps to the WAIT marker in the TODO editor, creating it if the session doesn't have one.
2. Type the next instruction there. It's a plain Markdown buffer — write it over several sittings if you like, while Claude works on the last one.
3. **Cmd-Enter** sends the draft to the Claude terminal, files it as `### Message N`, and stamps `> last=`.
4. **Cmd-P** to move to another session while that one runs. Your panes, cursor, and terminal scrollback come back when you return.
5. Sessions that printed something while you were away carry a red `*` in the picker.

### Navigate Code

1. **Cmd-O** → fuzzy search over the project's git-tracked and untracked files (`M` = modified, `R` = recently opened), or jump straight to one of the five pane views.
2. **Cmd-Shift-O** → tree-sitter symbol outline for the focused Code pane, or the heading outline for a TODO pane.
3. **Cmd-F** → fuzzy line search within the focused editor.
4. Edits made outside jc are picked up automatically; if you have unsaved edits, jc three-way merges them and only asks when the merge conflicts.

### Manage Sessions

1. `jc .` from a repo to register a project. A second `jc .` routes to the running instance over the IPC socket.
2. **Cmd-Shift-P** lists what you can start in the current project: dormant TODO.md sessions (`*`), a brand-new session (`+`), and Claude transcripts on disk that TODO.md doesn't know about (`~`).
3. **Cmd-P** switches between running sessions across every project, and adopts dormant ones.
4. **Cmd-Shift-Backspace** in the session picker toggles `[D]` on a session's heading, so it stays in TODO.md but no longer starts with jc.

## Contributing

PRs welcome. Preferably have your Claude open one against mine — I don't accept human-authored code.

## Further Reading

- [Don't Wait for Claude](https://jeapostrophe.github.io/tech/jc-workflow/) — Article explaining the workflow philosophy behind jc

- [PLAN.md](PLAN.md) — Task checklist
- [DESIGN.md](DESIGN.md) — Design principles, why not an editor plugin, remote workflow philosophy
- [ARCH.md](ARCH.md) — Implementation details: session lifecycle, terminal pipeline, hooks, TODO.md format

## Star History

<a href="https://star-history.com/#jeapostrophe/jc&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=jeapostrophe/jc&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=jeapostrophe/jc&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=jeapostrophe/jc&type=Date" />
  </picture>
</a>
