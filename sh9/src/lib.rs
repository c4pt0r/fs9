//! sh9 - Interactive shell for the FS9 distributed filesystem
//!
//! This crate provides:
//! - A POSIX-like shell with bash-compatible syntax
//! - sh9script: A scripting language with variables, functions, and control flow
//! - Integration with FS9 for all file operations
//! - Built-in commands (ls, cat, grep, etc.) implemented natively

pub mod ast;
pub mod error;
pub mod eval;
pub mod help;
pub mod lexer;
pub mod parser;
pub mod shell;

pub use error::{Sh9Error, Sh9Result};
pub use parser::parse;
pub use shell::{BuiltinFn, CapturedOutput, Shell, ShellBuilder};
