use crate::qr;
use gpui::*;
use gpui_component::ActiveTheme;

actions!(mobile_qr, [DismissQr]);

pub fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("escape", DismissQr, Some("MobileQrView")),
    KeyBinding::new("cmd-w", DismissQr, Some("MobileQrView")),
  ]);
}

pub enum MobileQrEvent {
  Dismissed,
}

pub struct MobileQrView {
  grid: Vec<Vec<bool>>,
  focus: FocusHandle,
  host: String,
  port: u16,
}

impl MobileQrView {
  pub fn new(payload: qr::QrPayload, _window: &mut Window, cx: &mut Context<Self>) -> Self {
    let grid = qr::generate_qr(&payload);
    Self { grid, focus: cx.focus_handle(), host: payload.host, port: payload.port }
  }

  fn dismiss(&mut self, _: &DismissQr, _window: &mut Window, cx: &mut Context<Self>) {
    cx.emit(MobileQrEvent::Dismissed);
  }
}

impl Focusable for MobileQrView {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl EventEmitter<MobileQrEvent> for MobileQrView {}

impl Render for MobileQrView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let theme = cx.theme();
    let module_size = px(5.0);

    let qr_grid =
      div().flex().flex_col().children(self.grid.iter().map(|row| {
        div().flex().children(row.iter().map(|&dark| {
          div().size(module_size).bg(if dark { gpui::black() } else { gpui::white() })
        }))
      }));

    div()
      .id("mobile-qr-modal")
      .key_context("MobileQrView")
      .track_focus(&self.focus)
      .on_action(cx.listener(Self::dismiss))
      .w(px(350.0))
      .bg(theme.background)
      .border_1()
      .border_color(theme.border)
      .rounded_lg()
      .shadow_lg()
      .p_4()
      .flex()
      .flex_col()
      .items_center()
      .gap_3()
      .child(
        div()
          .text_sm()
          .font_weight(FontWeight::SEMIBOLD)
          .text_color(theme.foreground)
          .child("Mobile Pairing"),
      )
      .child(div().p_3().bg(gpui::white()).rounded_md().child(qr_grid))
      .child(
        div()
          .text_xs()
          .text_color(theme.muted_foreground)
          .child(format!("{}:{}", self.host, self.port)),
      )
      .child(
        div().text_xs().text_color(theme.muted_foreground).child("Scan with the jc companion app"),
      )
  }
}
