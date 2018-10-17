use futures::task::{self, Task};
use futures::{Async, Poll, Stream};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct NotifyCell<T: Clone> {
    observer: Option<NotifyCellObserver<T>>,
    inner: Arc<Mutex<Inner<T>>>,
}

#[derive(Clone, Debug)]
pub struct NotifyCellObserver<T: Clone> {
    last_polled_at: usize,
    inner: Arc<Mutex<Inner<T>>>,
}

#[derive(Debug)]
struct Inner<T: Clone> {
    value: T,
    last_written_at: usize,
    subscribers: Vec<Task>,
    dropped: bool,
}

impl<T: Clone> NotifyCell<T> {
    pub fn new(value: T) -> Self {
        NotifyCell {
            observer: None,
            inner: Arc::new(Mutex::new(Inner {
                value,
                last_written_at: 0,
                subscribers: Vec::new(),
                dropped: false,
            })),
        }
    }

    pub fn set(&self, value: T) {
        let mut inner = self.inner.lock().unwrap();
        inner.value = value;
        inner.last_written_at += 1;
        for subscriber in inner.subscribers.drain(..) {
            subscriber.notify();
        }
    }

    pub fn get(&self) -> T {
        self.inner.lock().unwrap().value.clone()
    }

    pub fn observe(&self) -> NotifyCellObserver<T> {
        let inner = self.inner.lock().unwrap();
        NotifyCellObserver {
            last_polled_at: inner.last_written_at,
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone> NotifyCellObserver<T> {
    pub fn get(&self) -> T {
        self.inner.lock().unwrap().value.clone()
    }
}

impl<T: Clone> Stream for NotifyCellObserver<T> {
    type Item = T;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut inner = self.inner.lock().unwrap();

        if inner.dropped {
            Ok(Async::Ready(None))
        } else if self.last_polled_at < inner.last_written_at {
            self.last_polled_at = inner.last_written_at;
            Ok(Async::Ready(Some(inner.value.clone())))
        } else {
            inner.subscribers.push(task::current());
            Ok(Async::NotReady)
        }
    }
}

impl<T: Clone> Drop for NotifyCell<T> {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        inner.dropped = true;
        for subscriber in inner.subscribers.drain(..) {
            subscriber.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate futures_cpupool;
    extern crate rand;

    use self::futures_cpupool::CpuPool;
    use self::rand::Rng;
    use super::*;
    use futures::Future;
    use std::collections::BTreeSet;

    #[test]
    fn test_notify() {
        let generated_values = rand::thread_rng()
            .gen_iter::<u16>()
            .take(1000)
            .collect::<BTreeSet<_>>();

        let mut generated_values_iter = generated_values.clone().into_iter();
        let cell = NotifyCell::new(generated_values_iter.next().unwrap());

        let num_threads = 100;
        let pool = CpuPool::new(num_threads);

        let cpu_futures = (0..num_threads)
            .map(|_| pool.spawn(cell.observe().collect()))
            .collect::<Vec<_>>();

        for value in generated_values_iter {
            cell.set(value);
        }
        drop(cell); // Dropping the cell terminates the stream.

        for future in cpu_futures {
            let observed_values = future.wait().unwrap();
            let mut iter = observed_values.iter().peekable();

            while let Some(value) = iter.next() {
                assert!(generated_values.contains(value));
                if let Some(next_value) = iter.peek() {
                    assert!(value < next_value);
                }
            }
        }
    }
}
