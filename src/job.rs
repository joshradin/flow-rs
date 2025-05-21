//! Allows for defining an action, similar to [`ActionWithState`](crate::actions::ActionWithState) via
//! a trait object.

use crate::actions::{ActionWithState, IntoAction, stateful_action};
use std::marker::PhantomData;

/// This specifies a type that can be run as part of a Job, that has no input
pub trait NoInputJob<R: Send>: Send {
    /// Runs this job
    fn execute(&mut self) -> R;
}

/// This specifies a type that can be run as part of a Job that has input
pub trait Job<T: Send, R: Send>: Send {
    /// Runs this job with the given input
    fn execute(&mut self, input: T) -> R;
}

#[doc(hidden)]
pub struct JobMarker<T, R>(PhantomData<(T, R)>);
#[doc(hidden)]
pub struct NoInputJobMarker<R>(PhantomData<R>);

impl<T: Send + 'static, R: Send + 'static, J> IntoAction<T, R, JobMarker<T, R>> for J
where
    J: Job<T, R> + 'static,
{
    type Action = ActionWithState<J, T, R>;

    fn into_action(self) -> Self::Action {
        stateful_action(self, |state: &mut J, i: T| state.execute(i))
    }
}

impl<R: Send + 'static, J> IntoAction<(), R, NoInputJobMarker<R>> for J
where
    J: NoInputJob<R> + 'static,
{
    type Action = ActionWithState<J, (), R>;

    fn into_action(self) -> Self::Action {
        stateful_action(self, |state: &mut J| state.execute())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Flow;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_job_with_flow() {
        #[derive(Debug, Default)]
        struct TestJob {
            value: Arc<AtomicBool>,
        }

        impl NoInputJob<()> for TestJob {
            fn execute(&mut self) {
                let _ =
                    self.value
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed);
            }
        }

        let mut flow: Flow = Flow::default();
        let test = TestJob::default();
        let value = test.value.clone();
        let t = flow.create("test", test);
        flow.run().unwrap();
        assert!(
            value.load(Ordering::SeqCst),
            "should have run and set the value"
        );
    }
}
