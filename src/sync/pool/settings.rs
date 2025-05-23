//! Thread pool settings

use std::time::Duration;

#[derive(Clone, Copy)]
pub struct ThreadPoolSettings {
    core_size: usize,
    max_size: usize,
    keep_alive_timeout: Duration,
}

impl Default for ThreadPoolSettings {
    fn default() -> Self {
        Self {
            core_size: num_cpus::get(),
            max_size: num_cpus::get(),
            keep_alive_timeout: Duration::from_secs(5),
        }
    }
}

impl ThreadPoolSettings {
    pub fn new(core_size: usize, max_size: usize, keep_alive_timeout: Duration) -> Self {
        Self {
            core_size,
            max_size,
            keep_alive_timeout,
        }
    }

    pub fn core_size(&self) -> usize {
        self.core_size
    }

    pub fn max_size(&self) -> usize {
        self.max_size
    }

    pub fn keep_alive_timeout(&self) -> Duration {
        self.keep_alive_timeout
    }
}
