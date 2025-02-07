//! The native `fs` module for the [Rune Language].
//!
//! [Rune Language]: https://rune-rs.github.io
//!
//! ## Usage
//!
//! Add the following to your `Cargo.toml`:
//!
//! ```toml
//! rune-modules = { version = "0.12.1", features = ["fs"] }
//! ```
//!
//! Install it into your context:
//!
//! ```rust
//! # fn main() -> rune::Result<()> {
//! let mut context = rune::Context::with_default_modules()?;
//! context.install(&rune_modules::fs::module(true)?)?;
//! # Ok(())
//! # }
//! ```
//!
//! Use it in Rune:
//!
//! ```rust,ignore
//! fn main() {
//!     let file = fs::read_to_string("file.txt").await?;
//!     println(`{file}`);
//! }
//! ```

use std::io;
use tokio::fs;
use rune::{Module, ContextError};

/// Construct the `fs` module.
pub fn module(_stdio: bool) -> Result<Module, ContextError> {
    let mut module = Module::with_crate("fs");
    module.async_function(["read_to_string"], read_to_string)?;
    Ok(module)
}

async fn read_to_string(path: &str) -> io::Result<String> {
    fs::read_to_string(path).await
}
