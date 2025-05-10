use crate::backend::task::Data;
use crate::promise::MapPromise;
use crate::promise::{BoxPromise, GetPromise, PollPromise, Promise};
use parking_lot::Mutex;
use std::any::{type_name, Any};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

#[derive(Clone)]
pub struct Reusable {
    state: Arc<Mutex<ReusableState>>,
    cloner: Arc<dyn Fn(&Data) -> Data + Send + Sync>,
}

impl Debug for Reusable {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reusable").finish_non_exhaustive()
    }
}

enum ReusableState {
    Init(BoxPromise<'static, Data>),
    Data(Data),
}

impl Reusable {
    pub fn new<P: Promise + 'static>(promise: P) -> Self
    where
        P::Output: Clone + Send + 'static,
    {
        let first_promise =
            Box::new(promise.map(|promise| Box::new(promise) as Data)) as BoxPromise<Data>;
        let cloner = {
            let closure = move |data: &Data| -> Data {
                let downcast = data.downcast_ref::<P::Output>().unwrap_or_else(|| {
                    panic!("Failed to downcast data to {}", type_name::<P::Output>())
                });

                let cloned = downcast.clone();
                Box::new(cloned) as Data
            };
            Arc::from(Box::new(closure) as Box<dyn Fn(&Data) -> Data + Send + Sync>)
        };

        Reusable {
            state: Arc::new(Mutex::new(ReusableState::Init(first_promise))),
            cloner,
        }
    }
}

impl crate::promise::IntoPromise for Reusable {
    type Output = Box<dyn Any + Send>;
    type IntoPromise = IntoPromise;

    fn into_promise(self) -> Self::IntoPromise {
        IntoPromise { inner: self }
    }
}

/// The reusable promise
pub struct IntoPromise {
    inner: Reusable,
}

impl Promise for IntoPromise {
    type Output = Box<dyn Any + Send>;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        let Reusable { state, cloner } = &mut self.inner;
        let mut state = state.lock();
        match &mut *state {
            ReusableState::Init(promise) => match promise.poll() {
                PollPromise::Ready(ready) => {
                    let cloned = cloner(&ready);
                    *state = ReusableState::Data(ready);
                    PollPromise::Ready(cloned)
                }
                PollPromise::Pending => PollPromise::Pending,
            },
            ReusableState::Data(data) => {
                let cloned = cloner(data);
                PollPromise::Ready(cloned)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promise::Just;

    #[test]
    fn test_reusuable() {
        let reusable = Reusable::new(Just::new(123_i32));
        let a = *reusable.clone().get().downcast::<i32>().unwrap();
        let b = *reusable.get().downcast::<i32>().unwrap();
        assert_eq!(a, b);
    }
}
