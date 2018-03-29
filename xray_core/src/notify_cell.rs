use std::sync::{Arc, Weak};
use futures::{Async, Poll, Stream};
use futures::task::{self, Task};
use parking_lot::RwLock;

type Version = usize;

#[derive(Debug, Eq, PartialEq)]
pub enum TrySetError {
    ObserverDisconnected
}

#[derive(Debug)]
pub struct NotifyCell<T: Clone> {
    observer: Option<NotifyCellObserver<T>>,
    inner: Arc<RwLock<Inner<T>>>
}

pub struct WeakNotifyCell<T: Clone>(Weak<RwLock<Inner<T>>>);

#[derive(Debug)]
pub struct NotifyCellObserver<T: Clone> {
    last_polled_at: Version,
    inner: Arc<RwLock<Inner<T>>>,
}

#[derive(Debug)]
struct Inner<T: Clone> {
    value: T,
    last_written_at: Version,
    subscribers: Vec<Task>,
    dropped: bool
}

impl<T: Clone> NotifyCell<T> {
    pub fn new(value: T) -> Self {
        NotifyCell {
            observer: None,
            inner: Arc::new(RwLock::new(Inner {
                value,
                last_written_at: 0,
                subscribers: Vec::new(),
                dropped: false
            }))
        }
    }

    pub fn weak(value: T) -> (WeakNotifyCell<T>, NotifyCellObserver<T>) {
        let observer = NotifyCellObserver {
            last_polled_at: 0,
            inner: Arc::new(RwLock::new(Inner {
                value,
                last_written_at: 0,
                subscribers: Vec::new(),
                dropped: false
            }))
        };
        let weak_cell = WeakNotifyCell(Arc::downgrade(&observer.inner));
        (weak_cell, observer)
    }

    pub fn set(&self, value: T) {
        let mut inner = self.inner.write();
        inner.value = value;
        inner.last_written_at += 1;
        for subscriber in inner.subscribers.drain(..) {
            subscriber.notify();
        }
    }

    pub fn get(&self) -> T {
        self.inner.read().value.clone()
    }

    pub fn observe(&self) -> NotifyCellObserver<T> {
        let inner = self.inner.read();
        NotifyCellObserver {
            last_polled_at: inner.last_written_at,
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone> WeakNotifyCell<T> {
    pub fn try_set(&self, value: T) -> Result<(), TrySetError> {
        let inner = self.0.upgrade().ok_or(TrySetError::ObserverDisconnected)?;
        let mut inner = inner.write();
        inner.value = value;
        inner.last_written_at += 1;
        for subscriber in inner.subscribers.drain(..) {
            subscriber.notify();
        }
        Ok(())
    }
}

impl<T: Clone> NotifyCellObserver<T> {
    pub fn get(&self) -> T {
        self.inner.read().value.clone()
    }
}

impl<T: Clone> Stream for NotifyCellObserver<T> {
    type Item = T;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let inner = self.inner.upgradable_read();

        if inner.dropped {
            Ok(Async::Ready(None))
        } else if self.last_polled_at < inner.last_written_at {
            self.last_polled_at = inner.last_written_at;
            Ok(Async::Ready(Some(inner.value.clone())))
        } else {
            inner.upgrade().subscribers.push(task::current());
            Ok(Async::NotReady)
        }
    }
}

impl<T: Clone> Stream for NotifyCell<T> {
    type Item = T;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.observer.is_none() {
            self.observer = Some(self.observe());

            let inner = self.inner.read();
            if inner.last_written_at == 0 {
                // Release read lock before polling to avoid deadlocks.
                drop(inner);
                self.observer.as_mut().unwrap().poll()
            } else {
                Ok(Async::Ready(Some(inner.value.clone())))
            }
        } else {
            self.observer.as_mut().unwrap().poll()
        }
    }
}

impl<T: Clone> Drop for NotifyCell<T> {
    fn drop(&mut self) {
        let mut inner = self.inner.write();
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

    use super::*;
    use std::collections::BTreeSet;
    use futures::Future;
    use self::rand::Rng;
    use self::futures_cpupool::CpuPool;

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

    #[test]
    fn test_notify_cell_poll() {
        CpuPool::new_num_cpus().spawn_fn(|| -> Result<(), ()> {
            let mut cell = NotifyCell::new(1);
            assert_eq!(cell.poll(), Ok(Async::NotReady));

            cell.set(2);
            assert_eq!(cell.poll(), Ok(Async::Ready(Some(2))));
            assert_eq!(cell.poll(), Ok(Async::NotReady));

            let mut cell = NotifyCell::new(1);
            cell.set(2);
            assert_eq!(cell.poll(), Ok(Async::Ready(Some(2))));
            assert_eq!(cell.poll(), Ok(Async::NotReady));

            cell.set(3);
            assert_eq!(cell.poll(), Ok(Async::Ready(Some(3))));
            assert_eq!(cell.poll(), Ok(Async::NotReady));

            Ok(())
        }).wait().unwrap();
    }

    #[test]
    fn test_weak_notify_cell() {
        let (cell, observer) = NotifyCell::weak(1);
        assert_eq!(observer.get(), 1);

        assert_eq!(cell.try_set(2), Ok(()));
        assert_eq!(observer.get(), 2);

        assert_eq!(cell.try_set(3), Ok(()));
        assert_eq!(observer.get(), 3);

        drop(observer);
        assert_eq!(cell.try_set(4), Err(TrySetError::ObserverDisconnected));
    }
}
