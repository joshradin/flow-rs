use crate::backend::task::Data;
use crate::promise::MapPromise;
use crate::promise::{BoxPromise, PollPromise, Promise};
use parking_lot::Mutex;
use std::any::type_name;
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::sync::Arc;

pub struct Reusable<'lf, T, P = BoxPromise<'lf, T>>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    state: Arc<Mutex<ReusableState<'lf, T, P>>>,
    cloner: Arc<dyn Fn(&T) -> T + Send + Sync + 'lf>,
}

impl<'lf, T, P> Clone for Reusable<'lf, T, P>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            cloner: self.cloner.clone(),
        }
    }
}

impl<'lf, T, P> Debug for Reusable<'lf, T, P>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reusable").finish_non_exhaustive()
    }
}

enum ReusableState<'lf, T: 'lf, P: Promise<Output = T> + 'lf>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    Init(P, PhantomData<&'lf T>),
    Data(T),
}

impl<'lf, T: 'lf, P: Promise<Output = T> + 'lf> Reusable<'lf, T, P>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    pub fn new(promise: P) -> Self
    where
        T: Clone,
    {
        let cloner = {
            let closure = move |data: &T| -> T { data.clone() };
            Arc::from(Box::new(closure) as Box<dyn Fn(&T) -> T + Send + Sync + 'lf>)
        };

        Reusable {
            state: Arc::new(Mutex::new(ReusableState::Init(promise, PhantomData))),
            cloner,
        }
    }

    /// Converts this into a data promise. Panics if it can't take exclusive access over the interior.
    pub fn into_data(self) -> Reusable<'static, Data, BoxPromise<'static, Data>>
    where
        T: Clone + 'static,
        P: 'static,
    {
        let cloner = move |data: &Data| -> Data {
            let downcast = data
                .downcast_ref::<T>()
                .unwrap_or_else(|| panic!("Failed to downcast data to {}", type_name::<T>()));

            let cloned = downcast.clone();
            Box::new(cloned) as Data
        };
        let state_mutex =
            Arc::into_inner(self.state).expect("can not get exlusive access to interior");
        let state = Mutex::into_inner(state_mutex);
        match state {
            ReusableState::Init(promise, _) => {
                let first_promise =
                    Box::new(promise.map(|promise| Box::new(promise) as Data)) as BoxPromise<Data>;
                Reusable {
                    state: Arc::new(Mutex::new(ReusableState::Init(first_promise, PhantomData))),
                    cloner: Arc::new(cloner),
                }
            }
            ReusableState::Data(data) => Reusable {
                state: Arc::new(Mutex::new(ReusableState::Data(
                    Box::new(data.clone()) as Data
                ))),
                cloner: Arc::new(cloner),
            },
        }
    }
}

impl<'lf, T: Send + Sync + 'lf, P: Promise<Output = T> + 'lf> crate::promise::IntoPromise for Reusable<'lf, T, P>
where
    T: Send + Sync + 'lf,
    P: Promise<Output = T> + 'lf,
{
    type Output = T;
    type IntoPromise = IntoPromise<'lf, T, P>;

    fn into_promise(self) -> Self::IntoPromise {
        IntoPromise { inner: self }
    }
}

/// The reusable promise
pub struct IntoPromise<'lf, T, P: Promise<Output = T> + 'lf>
where T: Send + Sync + 'lf,
{
    inner: Reusable<'lf, T, P>,
}

impl<'lf, T: Send + Sync + 'lf, P: Promise<Output = T> + 'lf> Promise for IntoPromise<'lf, T, P> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        let Reusable { state, cloner } = &mut self.inner;
        let mut state = state.lock();
        match &mut *state {
            ReusableState::Init(promise, _) => match promise.poll() {
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
    use crate::promise::{GetPromise, Just};

    #[test]
    fn test_reusuable() {
        let reusable = Reusable::new(Just::new(123_i32));
        let a = reusable.clone().get();
        let b = reusable.get();
        assert_eq!(a, b);
    }
}
