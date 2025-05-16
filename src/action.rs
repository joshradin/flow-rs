//! Actions maps an input to an output

use std::marker::PhantomData;
use crate::backend::job::InputFlavor;

/// A step
pub trait Action: Send {
    type Input: Send;
    type Output: Send;

    /// Runs this step
    fn apply(&mut self, input: Self::Input) -> Self::Output;

    /// Maps the output of this flow
    fn map<U, F>(self, f: F) -> Map<Self, U, F>
    where
        Self: Sized,
        F: FnMut(Self::Output) -> U + Send,
        U: Send,
    {
        Map {
            flow: self,
            map: f,
            _marker: Default::default(),
        }
    }

    /// Chains two flows together
    fn chain<F>(self, other: F) -> Chain<Self, F>
    where
        Self: Sized,
        F: Action<Input = Self::Output>,
    {
        Chain {
            flow1: self,
            flow2: other,
        }
    }
}

pub trait Runnable: Action<Input = (), Output = ()> + Sized {
    fn run(&mut self) {
        self.apply(());
    }
}
impl<A: Action<Input = (), Output = ()>> Runnable for A {}
pub trait Consumer<T: Send>: Action<Input = T, Output = ()> + Sized {
    fn accept(&mut self, t: T) {
        self.apply(t);
    }
}
impl<T: Send, A: Action<Input = T, Output = ()>> Consumer<T> for A {}
pub trait Supplier<T: Send>: Action<Input = (), Output = T> + Sized {
    fn get(&mut self) -> T {
        self.apply(())
    }
}
impl<T: Send, A: Action<Input = (), Output = T>> Supplier<T> for A {}

pub type BoxAction<'lf, I, O> = Box<dyn Action<Input = I, Output = O> + 'lf>;

impl<'a, I, O> Action for Box<dyn Action<Input = I, Output = O> + 'a>
where
    I: Send + 'a,
    O: Send + 'a,
{
    type Input = I;
    type Output = O;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        (**self).apply(input)
    }
}

/// A step that maps the output of an inner
pub struct Map<F, U, M>
where
    F: Action,
    U: Send,
    M: FnMut(F::Output) -> U + Send,
{
    flow: F,
    map: M,
    _marker: PhantomData<U>,
}

impl<F: Action, U: Send, M: FnMut(F::Output) -> U + Send> Action for Map<F, U, M> {
    type Input = F::Input;
    type Output = U;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        let mid = self.flow.apply(input);
        (self.map)(mid)
    }
}

/// Chains two flows together
pub struct Chain<F1, F2>
where
    F1: Action,
    F2: Action<Input = F1::Output>,
{
    flow1: F1,
    flow2: F2,
}

impl<F1, F2> Action for Chain<F1, F2>
where
    F1: Action,
    F2: Action<Input = F1::Output>,
{
    type Input = F1::Input;
    type Output = F2::Output;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        let mid = self.flow1.apply(input);
        self.flow2.apply(mid)
    }
}

/// A flow based on a function
pub struct FnAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    f: F,
    _marker: PhantomData<(I, O)>,
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
}

/// Creates an action from a function
pub fn action<I, O, F>(f: F) -> FnAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    FnAction {
        f,
        _marker: PhantomData,
    }
}

/// A flow based on a function that can only run once
pub struct FnOnceAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnOnce(I) -> O + Send,
{
    f: Option<F>,
    _marker: PhantomData<(I, O)>,
}

impl<I, O, F> Action for FnOnceAction<I, O, F>
where
    I: Send,
    O: Send,
    F: FnOnce(I) -> O + Send,
{
    type Input = I;
    type Output = O;

    fn apply(&mut self, input: Self::Input) -> Self::Output {
        match self.f.take() {
            None => {
                panic!("This action can not be run twice")
            }
            Some(f) => f(input),
        }
    }
}

pub trait IntoAction<In, Out, Marker>: Sized {
    type Action: Action<Input = In, Output = Out>;

    fn into_action(self) -> (InputFlavor, Self::Action);
}

pub struct ProducerIntoAction<R>(PhantomData<R>);
impl<R: Send + 'static, F: FnMut() -> R + Send + 'static> IntoAction<(), R, ProducerIntoAction<R>>
    for F
{
    type Action = BoxAction<'static, (), R>;

    fn into_action(mut self) -> (InputFlavor, Self::Action) {
        (InputFlavor::None, Box::new(action(move |_: ()| self())) as BoxAction<'static, (), R>)
    }
}

pub struct FunctionIntoAction<T, R>(PhantomData<(T, R)>);
impl<T: Send + 'static, R: Send + 'static, F: FnMut(T) -> R + Send + 'static>
    IntoAction<T, R, FunctionIntoAction<T, R>> for F
{
    type Action = FnAction<T, R, F>;

    fn into_action(self) -> (InputFlavor, Self::Action) {
        (InputFlavor::Single, action(self))
    }
}

impl<A: Action> IntoAction<A::Input, A::Output, ()> for A {
    type Action = A;

    fn into_action(self) ->(InputFlavor, Self::Action){
        (InputFlavor::Single, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closure_action() {
        let mut action = action(|i: i32| i * i);
        assert_eq!(action.apply(1), 1);
        assert_eq!(action.apply(2), 4);
    }

    #[test]
    fn test_fn_action() {
        #[inline]
        fn square(i: i32) -> i32 {
            i * i
        }
        assert_eq!(action(square).apply(1), 1);
    }

    #[test]
    fn test_map_action() {
        let mut action = action(|i: i32| i).map(|i| i * i);
        assert_eq!(action.apply(1), 1);
        assert_eq!(action.apply(2), 4);
    }

    #[test]
    fn test_chain_action() {
        let mut action = action(|i: i32| i).chain(action(|i| i * i));
        assert_eq!(action.apply(1), 1);
        assert_eq!(action.apply(2), 4);
    }

    #[test]
    fn test_box_action() {
        let mut action: BoxAction<i32, i32> =
            Box::new(Box::new(action(|i: i32| i)).chain(action(|i| i * i)));
        assert_eq!(action.apply(1), 1);
        assert_eq!(action.apply(2), 4);
    }

    #[test]
    fn test_into_action() {
        let (_, mut io_action) = (|i: i32| i * i).into_action();
        let v = io_action.apply(123);
    }
}
