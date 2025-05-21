//! helper types and functions

use crate::actions::{Action, IntoAction};
use crate::backend::job::InputFlavor;
use std::marker::PhantomData;

/// Creates an action from a function
pub fn action<I, O, A: IntoAction<I, O, Marker>, Marker>(f: A) -> A::Action
where
    I: Send,
    O: Send,
{
    f.into_action()
}

/// A flow based on a function
pub struct FnAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    f: F,
    input_flavor: InputFlavor,
    _marker: PhantomData<(I, O)>,
}

impl<I, O, F> FnAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    pub(crate) fn new(input_flavor: InputFlavor, f: F) -> Self {
        Self {
            f,
            input_flavor,
            _marker: PhantomData,
        }
    }
}

impl<I, O, F> Action for FnAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    type Input = I;
    type Output = O;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        (self.f)(input)
    }

    fn input_flavor(&self) -> InputFlavor {
        self.input_flavor
    }
}

pub fn stateful_action<T: Send, A: IntoStatefulAction<T, Marker>, Marker>(
    state: T,
    action: A,
) -> ActionWithState<T, A::In, A::Out> {
    action.into_action(state)
}

type BoxedStatefulActionFn<T, I, O> = Box<dyn FnMut(&mut T, I) -> O + Send>;

/// An action with a state
pub struct ActionWithState<T: Send, I: Send, O: Send> {
    input_flavor: InputFlavor,
    state: T,
    f: BoxedStatefulActionFn<T, I, O>,
    _marker: PhantomData<(I, O)>,
}

impl<T: Send, I: Send, O: Send> Action for ActionWithState<T, I, O> {
    type Input = I;
    type Output = O;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        let Self { state, f, .. } = self;
        f(state, input)
    }

    fn input_flavor(&self) -> InputFlavor {
        self.input_flavor
    }
}

pub struct ActionWithStateNoInputMarker;

pub struct ActionWithStateMarker<I: Send>(PhantomData<I>);

pub trait IntoStatefulAction<T: Send, Marker> {
    type In: Send;
    type Out: Send;

    fn into_action(self, state: T) -> ActionWithState<T, Self::In, Self::Out>;
}

impl<O: Send + 'static, T: Send + 'static, F> IntoStatefulAction<T, ActionWithStateNoInputMarker>
    for F
where
    F: FnMut(&mut T) -> O + Send + 'static,
{
    type In = ();
    type Out = O;

    fn into_action(mut self, state: T) -> ActionWithState<T, Self::In, Self::Out> {
        let f = Box::new(move |state: &mut T, _input: ()| -> O { self(state) });
        ActionWithState {
            input_flavor: InputFlavor::None,
            state,
            f: f as BoxedStatefulActionFn<T, (), O> ,
            _marker: Default::default(),
        }
    }
}

impl<I: Send + 'static, O: Send + 'static, T: Send + 'static, F>
    IntoStatefulAction<T, ActionWithStateMarker<I>> for F
where
    F: FnMut(&mut T, I) -> O + Send + 'static,
{
    type In = I;
    type Out = O;

    fn into_action(mut self, state: T) -> ActionWithState<T, Self::In, Self::Out> {
        let f = Box::new(move |state: &mut T, input: Self::In| -> O { self(state, input) });
        ActionWithState {
            input_flavor: InputFlavor::Single,
            state,
            f: f as BoxedStatefulActionFn<T, I, O> ,
            _marker: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::actions::{Action, IntoStatefulAction, stateful_action};
    use crate::{Flow, FlowsInto, InputFlavor};

    #[test]
    fn test_create_stateful_action() {
        let sts = stateful_action(32_i32, |t: &mut i32| {});
        assert_eq!(sts.input_flavor(), InputFlavor::None);
        let sts = stateful_action(32_i32, |t: &mut i32, i: String| {});
        assert_eq!(sts.input_flavor(), InputFlavor::Single);
    }

    #[test]
    fn test_stateful_action_as_action() {
        let no_input = stateful_action(32_i32, |t: &mut i32| "hello".to_string());
        let some_input = stateful_action(32_i32, |t: &mut i32, s: String| s.into_boxed_str());
        let mut flow: Flow = Flow::new();
        let job_reference1 = flow.create("no_input", no_input);
        let job_reference2 = flow.create("some_input", some_input);
        assert!(job_reference1.flows_into(job_reference2).is_ok());
    }
}
