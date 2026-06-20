#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {{
        nun::println!(
            "[Nanami][{:>6}] {}",
            "INFO",
            format_args!($($arg)*)
        );
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
