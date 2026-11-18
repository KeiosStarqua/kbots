//! Herdr Core framed terminal attachment and VT screen support.
mod hc_client;
mod screen;

pub use hc_client::{HcClient, TerminalEvent};
pub use screen::ScreenCell;
