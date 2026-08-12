use core::sync::atomic::{AtomicBool, Ordering};

static INFO_LOG_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_info_enabled(enabled: bool) {
    INFO_LOG_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn info_enabled() -> bool {
    INFO_LOG_ENABLED.load(Ordering::Relaxed)
}

#[macro_export]
macro_rules! force_info {
    ($($arg:tt)*) => {{
        nun::println!(
            "[Nanami][{:>6}] {}",
            "INFO",
            format_args!($($arg)*)
        );
    }};
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{
        if $crate::nanami_utils::log::info_enabled() {
            nun::println!(
                "[Nanami][{:>6}] {}",
                "INFO",
                format_args!($($arg)*)
            );
        }
    }};
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {{
        nun::println!(
            "[Nanami][{}{:>6}\x1b[0m] {}",
            "\x1b[31m",
            "ERROR",
            format_args!($($arg)*)
        );
    }};
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {{
        nun::println!(
            "[Nanami][{}{:>6}\x1b[0m] {}",
            "\x1b[38;5;208m",
            "WARN",
            format_args!($($arg)*)
        );
    }};
}

// only debug build
#[cfg(debug_assertions)]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        nun::println!(
            "[Nanami][{}{:>6}\x1b[0m] {}",
            "\x1b[34m",
            "DEBUG",
            format_args!($($arg)*)
        );
    }};
}

#[cfg(not(debug_assertions))]
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {};
}
