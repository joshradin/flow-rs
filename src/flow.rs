//! A step within a flow

use std::marker::PhantomData;

/// A step
pub trait Flow: Send {
    type Input: Send;
    type Output: Send;

    /// Runs this step
    fn run(&mut self, input: Self::Input) -> Self::Output;

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
        F: Flow<Input = Self::Output>,
    {
        Chain {
            flow1: self,
            flow2: other,
        }
    }
}

pub type BoxFlow<'lf, I, O> = Box<dyn Flow<Input = I, Output = O> + 'lf>;

impl<'a, I, O> Flow for Box<dyn Flow<Input = I, Output = O> + 'a>
where
    I: Send + 'a,
    O: Send + 'a,
{
    type Input = I;
    type Output = O;

    fn run(&mut self, input: Self::Input) -> Self::Output {
        (**self).run(input)
    }
}


/// A step that maps the output of an inner
pub struct Map<F, U, M>
where
    F: Flow,
    U: Send,
    M: FnMut(F::Output) -> U + Send,
{
    flow: F,
    map: M,
    _marker: PhantomData<U>,
}

impl<F: Flow, U: Send, M: FnMut(F::Output) -> U + Send> Flow for Map<F, U, M> {
    type Input = F::Input;
    type Output = U;

    fn run(&mut self, input: Self::Input) -> Self::Output {
        let mid = self.flow.run(input);
        (self.map)(mid)
    }
}

/// Chains two flows together
pub struct Chain<F1, F2>
where
    F1: Flow,
    F2: Flow<Input = F1::Output>,
{
    flow1: F1,
    flow2: F2,
}

impl<F1, F2> Flow for Chain<F1, F2>
where
    F1: Flow,
    F2: Flow<Input = F1::Output>,
{
    type Input = F1::Input;
    type Output = F2::Output;

    fn run(&mut self, input: Self::Input) -> Self::Output {
        let mid = self.flow1.run(input);
        self.flow2.run(mid)
    }
}

/// A flow based on a function
pub struct FnFlow<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    f: F,
    _marker: PhantomData<(I, O)>,
}

impl<I, O, F> Flow for FnFlow<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    type Input = I;
    type Output = O;

    fn run(&mut self, input: Self::Input) -> Self::Output {
        (self.f)(input)
    }
}

/// Creates a flow from a function
pub fn flow<I, O, F>(f: F) -> FnFlow<I, O, F>
where
    I: Send,
    O: Send,
    F: FnMut(I) -> O + Send,
{
    FnFlow {
        f,
        _marker: PhantomData,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closure_flow() {
        let mut flow = flow(|i: i32| i * i);
        assert_eq!(flow.run(1), 1);
        assert_eq!(flow.run(2), 4);
    }


    #[test]
    fn test_fn_flow() {
        #[inline]
        fn square(i: i32) -> i32 {
            i * i
        }
        assert_eq!(flow(square).run(1), 1);
    }

    #[test]
    fn test_map_flow() {
        let mut flow = flow(|i: i32| i).map(|i| i * i);
        assert_eq!(flow.run(1), 1);
        assert_eq!(flow.run(2), 4);
    }

    #[test]
    fn test_chain_flow() {
        let mut flow = flow(|i: i32| i).chain(flow(|i| i * i));
        assert_eq!(flow.run(1), 1);
        assert_eq!(flow.run(2), 4);
    }

    #[test]
    fn test_box_flow() {
        let mut flow: BoxFlow<i32, i32> =
            Box::new(Box::new(flow(|i: i32| i)).chain(flow(|i| i * i)));
        assert_eq!(flow.run(1), 1);
        assert_eq!(flow.run(2), 4);
    }
}
