//! Tracing/log-level setup for the `yupana` CLI, split out of `cli` for size
//! (yupana #83). See `cli_analyze` for why this is a child module.

use std::io;

use tracing_subscriber::EnvFilter;

///
/// `RUST_LOG` wins when set — the conventional Rust escape hatch, and it can
/// target specific modules, which a boolean flag cannot. Absent it, `--verbose`
/// raises the default from `info` to `debug`. So precedence is
/// `RUST_LOG` > `--verbose` > the `info` default.
pub fn init_tracing(verbose: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_log_level(verbose)));
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(filter)
        .try_init();
}

/// The default tracing level when `RUST_LOG` is unset.
fn default_log_level(verbose: bool) -> &'static str {
    if verbose {
        "debug"
    } else {
        "info"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_raises_the_default_log_level() {
        assert_eq!(default_log_level(false), "info");
        assert_eq!(default_log_level(true), "debug");
    }
}
