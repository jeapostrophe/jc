use gpui::*;
use gpui_component::ActiveTheme;

actions!(keybinding_help, [DismissHelp]);

pub fn init(cx: &mut App) {
  cx.bind_keys([KeyBinding::new("escape", DismissHelp, Some("KeybindingHelp"))]);
}

struct Section {
  title: &'static str,
  bindings: &'static [(&'static str, &'static str)],
}

const SECTIONS: &[Section] = &[
  Section {
    title: "Global",
    bindings: &[
      ("Cmd-1", "1-pane layout"),
      ("Cmd-2", "2-pane layout"),
      ("Cmd-3", "3-pane layout"),
      ("Cmd-.", "View picker"),
      ("Cmd-[", "Focus previous pane"),
      ("Cmd-]", "Focus next pane"),
      ("Cmd-P", "Session picker"),
      ("Cmd-O", "File picker"),
      ("Cmd-T", "Context picker"),
      ("Cmd-Shift-O", "Git log picker"),
      ("Cmd-F", "Search lines"),
      ("Cmd-K", "Comment panel"),
      ("Cmd-Shift-K", "Snippet picker"),
      ("Cmd-S", "Save file"),
      ("Cmd-Enter", "Send to terminal"),
      ("Cmd-;", "Next problem"),
      ("Cmd-:", "Problem picker"),
      ("Cmd-Shift-C", "Copy reply (/copy)"),
      ("Cmd-?", "Keybinding help"),
      ("Cmd-Shift-E", "Open in external editor"),
      ("Cmd-W", "Close window"),
      ("Cmd-M", "Minimize window"),
      ("Cmd-Q", "Quit"),
    ],
  },
  Section {
    title: "View-Specific",
    bindings: &[
      ("Cmd-R", "Reload / Mark reviewed (Code/Diff)"),
      ("Cmd-C", "Copy selection (Terminal)"),
      ("Cmd-V", "Paste (Terminal)"),
      ("Cmd-=/+", "Increase font size (Terminal)"),
      ("Cmd--", "Decrease font size (Terminal)"),
      ("Cmd-0", "Reset font size (Terminal)"),
    ],
  },
  Section {
    title: "Picker",
    bindings: &[
      ("Enter", "Confirm"),
      ("Escape", "Cancel"),
      ("Down / Ctrl-N", "Next item"),
      ("Up / Ctrl-P", "Previous item"),
      ("Cmd-Shift-Bksp", "Remove session"),
    ],
  },
  Section {
    title: "Comment Panel",
    bindings: &[("Cmd-Enter", "Submit comment"), ("Escape", "Dismiss")],
  },
];

pub struct KeybindingHelp {
  focus: FocusHandle,
}

impl KeybindingHelp {
  pub fn new(cx: &mut Context<Self>) -> Self {
    Self { focus: cx.focus_handle() }
  }
}

impl Focusable for KeybindingHelp {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

fn render_section(section: &Section, theme: &gpui_component::Theme) -> Div {
  let mut rows = div().flex().flex_col().gap_0p5();
  for &(key, action) in section.bindings {
    rows = rows.child(
      div()
        .flex()
        .flex_row()
        .gap_4()
        .child(
          div()
            .w(px(140.0))
            .flex_shrink_0()
            .text_right()
            .text_color(theme.accent_foreground)
            .child(key),
        )
        .child(div().text_color(theme.foreground).child(action)),
    );
  }

  div()
    .flex()
    .flex_col()
    .gap_1()
    .child(
      div()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.muted_foreground)
        .child(section.title),
    )
    .child(rows)
}

impl Render for KeybindingHelp {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();

    // Left column: Global bindings. Right column: everything else.
    let left = div().flex().flex_col().gap_4().child(render_section(&SECTIONS[0], theme));

    let mut right = div().flex().flex_col().gap_4();
    for section in &SECTIONS[1..] {
      right = right.child(render_section(section, theme));
    }

    div()
      .id("keybinding-help")
      .key_context("KeybindingHelp")
      .track_focus(&self.focus)
      .absolute()
      .inset_0()
      .flex()
      .items_center()
      .justify_center()
      .bg(theme.background.opacity(0.85))
      .on_action(cx.listener(|_, _: &DismissHelp, _window, cx| cx.emit(DismissHelpEvent)))
      .child(
        div()
          .flex()
          .flex_col()
          .w(px(820.0))
          .p_6()
          .rounded_lg()
          .bg(theme.secondary)
          .border_1()
          .border_color(theme.border)
          .text_sm()
          .child(
            div()
              .text_base()
              .font_weight(FontWeight::BOLD)
              .text_color(theme.foreground)
              .mb_4()
              .child("Keybindings"),
          )
          .child(div().flex().flex_row().gap_8().child(left).child(right)),
      )
  }
}

pub struct DismissHelpEvent;

impl EventEmitter<DismissHelpEvent> for KeybindingHelp {}
