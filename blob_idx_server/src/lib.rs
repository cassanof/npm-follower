pub mod blob;
pub mod http;
pub mod job;


/// Prints to stdout only if #cfg(debug_assertions) is set.
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            println!($($arg)*);
        }
    };
}

#[cfg(test)]
pub mod tests;
