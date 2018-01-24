use std::sync::{Arc, Mutex};
use futures::{Async, Poll, Stream};
use futures::task::{self, Task};

type Version = usize;

#[derive(Debug)]
pub struct NotifyCell<T: Clone> {
    inner: Arc<Mutex<Inner<T>>>,
}

pub struct NotifyCellObserver<T: Clone> {
    last_polled_at: Option<Version>,
    inner: Arc<Mutex<Inner<T>>>,
}

#[derive(Debug)]
struct Inner<T: Clone> {
    value: Option<T>,
    last_written_at: Version,
    subscribers: Vec<Task>,
}

impl<T: Clone> NotifyCell<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                value: None,
                last_written_at: 0,
                subscribers: Vec::new(),
            })),
        }
    }

    pub fn with_value(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                value: Some(value),
                last_written_at: 0,
                subscribers: Vec::new(),
            })),
        }
    }

    pub fn set(&self, value: T) {
        let mut inner = self.inner.lock().unwrap();
        inner.value = Some(value);
        inner.last_written_at += 1;
        for subscriber in inner.subscribers.drain(..) {
            subscriber.notify();
        }
    }

    pub fn observe(&self) -> NotifyCellObserver<T> {
        NotifyCellObserver {
            last_polled_at: None,
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone> Stream for NotifyCellObserver<T> {
    type Item = T;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut inner = self.inner.lock().unwrap();

        if let Some(value) = inner.value.as_ref().cloned() {
            if let Some(last_polled_at) = self.last_polled_at {
                if inner.last_written_at > last_polled_at {
                    self.last_polled_at = Some(inner.last_written_at);
                    Ok(Async::Ready(Some(value.clone())))
                } else {
                    inner.subscribers.push(task::current());
                    Ok(Async::NotReady)
                }
            } else {
                self.last_polled_at = Some(inner.last_written_at);
                Ok(Async::Ready(Some(value.clone())))
            }
        } else {
            inner.subscribers.push(task::current());
            Ok(Async::NotReady)
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate futures_cpupool;

    use super::*;
    use std::collections::BTreeSet;
    use futures::Future;
    use rand::{self, Rng};
    use self::futures_cpupool::CpuPool;

    #[test]
    fn test_notify() {
        let generated_values = rand::thread_rng()
            .gen_iter::<u16>()
            .take(1000)
            .collect::<BTreeSet<_>>();

        let mut generated_values_iter = generated_values.clone().into_iter();
        let cell = NotifyCell::new(Some(generated_values_iter.next().unwrap()));

        let num_threads = 100;
        let pool = CpuPool::new(num_threads);

        let cpu_futures = (0..num_threads).map(|_| {
            let observer = cell.observe();

            pool.spawn(
                observer
                    .take_while(|v| Ok(v.is_some()))
                    .map(|v| v.unwrap())
                    .collect()
            )
        }).collect::<Vec<_>>();

        for value in generated_values_iter {
            cell.set(Some(value));
        }
        cell.set(None);

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
