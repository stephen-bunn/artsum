pub mod display;
pub mod task;

use clap::Args;

/// Global flags that can be used with any command
#[derive(Debug, Clone, Copy, Args)]
pub struct GlobalFlags {
    #[arg(short, long, action = clap::ArgAction::Count, default_value_t = 0)]
    /// Verbosity level
    pub verbosity: u8,
    /// Enable debug output
    #[arg(long, default_value_t = false)]
    pub debug: bool,
    /// Disable color output
    #[arg(long, default_value_t = false)]
    pub no_color: bool,
    /// Disable progress output
    #[arg(long, default_value_t = false)]
    pub no_progress: bool,
    /// Disable display output
    #[arg(long, default_value_t = false)]
    pub no_display: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_flags_merge_verbosity() {
        let top_level = GlobalFlags {
            verbosity: 2,
            debug: false,
            no_color: false,
            no_progress: false,
            no_display: false,
        };

        let subcommand = GlobalFlags {
            verbosity: 1,
            debug: false,
            no_color: false,
            no_progress: false,
            no_display: false,
        };

        let merged = top_level.merge(&subcommand);
        assert_eq!(merged.verbosity, 2); // Should use the maximum
    }

    #[test]
    fn test_global_flags_merge_debug() {
        let top_level = GlobalFlags {
            verbosity: 0,
            debug: true,
            no_color: false,
            no_progress: false,
            no_display: false,
        };

        let subcommand = GlobalFlags {
            verbosity: 0,
            debug: false,
            no_color: false,
            no_progress: false,
            no_display: false,
        };

        let merged = top_level.merge(&subcommand);
        assert!(merged.debug); // Should be true if either is true
    }

    #[test]
    fn test_global_flags_merge_subcommand_precedence() {
        let top_level = GlobalFlags {
            verbosity: 0,
            debug: false,
            no_color: false,
            no_progress: false,
            no_display: false,
        };

        let subcommand = GlobalFlags {
            verbosity: 0,
            debug: true,
            no_color: true,
            no_progress: true,
            no_display: true,
        };

        let merged = top_level.merge(&subcommand);
        assert!(merged.debug);
        assert!(merged.no_color);
        assert!(merged.no_progress);
        assert!(merged.no_display);
    }
}
