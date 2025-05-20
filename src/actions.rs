//! Actions maps an input to an output

mod traits;
pub use traits::*;

mod helpers;
pub use helpers::*;

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
        let mut io_action = (|i: i32| i * i).into_action();
        let v = io_action.apply(123);
    }
}
