use crossbeam::channel::{Receiver, TryRecvError};
use crate::promise::{PollPromise, Promise};

pub struct RecvPromise<T> {
    receiver: Receiver<T>,
}

impl<T> RecvPromise<T> {
    pub fn new(receiver: Receiver<T>) -> Self {
        Self { receiver }
    }
}

impl<T: Send> Promise for RecvPromise<T> {
    type Output = T;

    fn poll(&mut self) -> PollPromise<Self::Output> {
        match self.receiver.try_recv() {
            Ok(t) => {
                PollPromise::Ready(t)
            }
            Err(TryRecvError::Empty) => {
                PollPromise::Pending
            }
            Err(TryRecvError::Disconnected) => {
                panic!("Promise channel is disconnected")
            }
        }
    }
}