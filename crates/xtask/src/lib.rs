//! The repository's own tools, run with `cargo xtask` (.cargo/config.toml).
//!
//! Nothing here prints with `print!` or `eprint!`, which panic once the
//! terminal has gone away (a closed pipe): [`say!`] and [`note!`] write
//! best effort, since the terminal is only a view of a run whose receipt is
//! written whatever happens to it.
#![deny(clippy::print_stdout, clippy::print_stderr)]

/// A line to stdout, best effort.
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stdout().lock(), $($arg)*);
    }};
}

/// A line to stderr, best effort.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let _ = ::std::writeln!(::std::io::stderr().lock(), $($arg)*);
    }};
}

pub mod proof;
