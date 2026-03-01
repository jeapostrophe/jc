# jc-terminal

Embedded terminal emulator for the jc GPUI application. Integrates `alacritty_terminal` (VTE parsing + terminal grid) with `portable-pty` (PTY spawning) and GPUI (rendering).

## Architecture

```
GPUI Window
  └─ TerminalView (Render + Focusable)
       ├─ TerminalState (Arc<Mutex<Term<EventProxy>>>)
       ├─ PtyHandle (Mutex<MasterPty>, Arc<Mutex<Writer>>)
       ├─ std::thread (blocking PTY reads → flume channel)
       └─ GPUI async task (receives bytes → process → cx.notify)
```

**Data flow:**
- **Input:** GPUI `KeyDownEvent` → `keystroke_to_bytes()` → `PtyHandle.write_all()`
- **Output:** PTY reader thread → flume channel → GPUI task → VTE `Processor.advance()` → `cx.notify()` → render
- **Render:** `canvas()` locks `Term`, iterates grid cells, paints backgrounds + text + cursor
- **Resize:** Detected in canvas paint closure, propagated to both PTY and terminal grid

## Modules

| Module | Purpose |
|---|---|
| `colors.rs` | `Palette` — 256 ANSI color palette, converts alacritty `Color` to GPUI `Hsla` |
| `input.rs` | `keystroke_to_bytes()` — GPUI keystrokes to terminal escape sequences |
| `terminal.rs` | `TerminalState` — wraps `Term<EventProxy>` + VTE `Processor` behind mutex |
| `pty.rs` | `PtyHandle` — spawns shell via `portable-pty`, provides write/resize |
| `render.rs` | `measure_cell()` + `paint_terminal()` — 3-pass cell painting |
| `view.rs` | `TerminalView` — GPUI `Render` + `Focusable`, owns all terminal state + I/O |

## Usage

```rust
use jc_terminal::TerminalView;

// Inside a GPUI window:
let view = cx.new(|cx| {
    TerminalView::new(Default::default(), None, window, cx)
});
```

## Example

```bash
cargo run -p jc-terminal --example terminal_window
```

Opens a 900x600 window with a full terminal emulator running your default shell.
