//! A custom oncelock implementation

use crossbeam::atomic::AtomicCell;
use std::cell::UnsafeCell;
use std::convert::Infallible;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::thread::yield_now;

/// A synchronization primitive which can nominally be written to only once.
///
/// Functionally identical to [`std::sync::OnceLock`] besides providing an optional or_init function
#[derive(Debug)]
pub struct OnceLock<T> {
    state: AtomicCell<OnceLockState>,
    value: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<T>,
}

impl<T> OnceLock<T> {
    #[inline]
    pub const fn new() -> Self {
        Self {
            state: AtomicCell::new(OnceLockState::Uninit),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.is_initialized() {
            Some(unsafe { self.get_unchecked() })
        } else {
            None
        }
    }

    #[allow(unused)]
    pub fn set(&self, value: T) -> Result<(), T> {
        match self.try_insert(value) {
            Ok(_) => Ok(()),
            Err((_, value)) => Err(value),
        }
    }

    /// Tries to insert the value
    #[inline]
    fn try_insert(&self, value: T) -> Result<&T, (&T, T)> {
        let mut value = Some(value);
        let res = self.get_or_init(|| value.take().unwrap());
        match value {
            Some(value) => Err((res, value)),
            None => Ok(res),
        }
    }

    #[inline]
    pub fn get_or_init<F>(&self, init: F) -> &T
    where
        F: FnOnce() -> T,
    {
        self.get_or_try_init(|| Ok::<T, Infallible>(init()))
            .unwrap()
    }

    #[inline]
    pub fn get_or_try_init<F, E>(&self, init: F) -> Result<&T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        if let Some(value) = self.get() {
            return Ok(value);
        };

        self.initialize(init)?;
        debug_assert!(self.is_initialized());
        Ok(unsafe { self.get_unchecked() })
    }

    #[inline]
    pub fn get_or_try_init_opt<F>(&self, init: F) -> Option<&T>
    where
        F: FnOnce() -> Option<T>,
    {
        self.get_or_try_init(|| init().ok_or(())).ok()
    }

    unsafe fn get_unchecked(&self) -> &T {
        unsafe { (*self.value.get()).assume_init_ref() }
    }
    fn initialize<F, E>(&self, f: F) -> Result<(), E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut res: Result<(), E> = Ok(());
        let slot = &self.value;

        loop {
            match self
                .state
                .compare_exchange(OnceLockState::Uninit, OnceLockState::Initializing)
            {
                Ok(_) => match f() {
                    Ok(ok) => {
                        unsafe { (*slot.get()).write(ok) };
                        self.state.store(OnceLockState::Initialized);
                        break;
                    }
                    Err(e) => {
                        res = Err(e);
                        self.state.store(OnceLockState::Uninit);
                        break;
                    }
                },
                Err(OnceLockState::Initializing) => {
                    yield_now() // block until ready
                }
                Err(OnceLockState::Initialized) => break,
                Err(OnceLockState::Uninit) => {
                    unreachable!()
                }
            }
        }

        res
    }

    /// Checks if this has been initialized
    fn is_initialized(&self) -> bool {
        self.state.load() == OnceLockState::Initialized
    }
}
unsafe impl<T: Send> Send for OnceLock<T> {}
unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
enum OnceLockState {
    #[default]
    Uninit,
    Initializing,
    Initialized,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_once_lock_set_once() {
        let once = OnceLock::<i32>::new();
        assert!(once.set(128).is_ok());
        assert!(once.set(256).is_err());
        assert_eq!(once.get(), Some(&128));
    }

    #[test]
    fn test_once_lock_try_set_opt() {
        let once = OnceLock::<i32>::new();
        assert!(once.get_or_try_init_opt(|| { None }).is_none());
        assert_eq!(once.get_or_try_init_opt(|| { Some(128) }), Some(&128));
        assert_eq!(once.get_or_try_init_opt(|| { Some(256) }), Some(&128));
    }
}
