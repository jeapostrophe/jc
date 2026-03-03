pub mod colors;
mod input;
mod pty;
mod render;
mod terminal;
mod view;

pub use colors::Palette;
pub use view::{TerminalConfig, TerminalView, init};
