//! Minimal Chrome DevTools Protocol client.
//!
//! Just enough of Target / Page / Runtime to drive a meeting page: launch a
//! Chromium-family browser that is already installed, attach a flattened
//! session, install a document-start script, expose bindings so page JS can
//! push data back, and evaluate expressions.
//!
//! This crate knows nothing about Teams or meetings. It downloads nothing —
//! it drives the Edge (or Chrome) already on the machine.
//!
//! ```no_run
//! # async fn demo() -> Result<(), xai_grok_cdp::CdpError> {
//! use xai_grok_cdp::{Browser, LaunchOptions};
//!
//! let browser = Browser::launch(&LaunchOptions::new("/tmp/profile")).await?;
//! let page = browser.new_page().await?;
//! page.expose_binding("turboChat").await?;
//! page.add_init_script("window.__turbo = true;").await?;
//! page.navigate("https://example.com").await?;
//! # Ok(())
//! # }
//! ```

mod conn;
mod error;
mod launch;
mod page;

pub use conn::{COMMAND_TIMEOUT, CdpEvent, Connection};
pub use error::{CdpError, Result};
pub use launch::{
    BROWSER_ENV, Headless, LaunchOptions, LaunchedBrowser, find_browser, launch,
    parse_endpoint_line,
};
pub use page::{BindingStream, Browser, Navigation, NavigationStream, Page};
