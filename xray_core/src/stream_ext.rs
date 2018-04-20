use futures::{Future, Poll, Stream};
use std::fmt::Debug;
use tokio_core::reactor;

pub trait StreamExt
where
    Self: Stream + Sized,
{
    fn wait_next(&mut self, reactor: &mut reactor::Core) -> Option<Self::Item>
    where
        Self::Item: Debug,
        Self::Error: Debug,
    {
        struct TakeOne<'a, S: 'a>(&'a mut S);

        impl<'a, S: 'a + Stream> Future for TakeOne<'a, S> {
            type Item = Option<S::Item>;
            type Error = S::Error;

            fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
                self.0.poll()
            }
        }

        reactor.run(TakeOne(self)).unwrap()
    }
}

impl<T: Stream> StreamExt for T {}
