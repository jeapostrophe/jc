pub mod colors;
mod input;
mod pty;
mod render;
mod settle;
mod terminal;
mod view;

pub use colors::Palette;
pub use settle::LaunchSettle;
pub use pty::ChildSpec;
pub use view::{ChildEvent, TerminalConfig, TerminalView, init};
