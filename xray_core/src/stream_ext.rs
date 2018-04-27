use futures::{Future, Poll, Stream};
use std::fmt::Debug;
use std::time;
use tokio_core::reactor;
use tokio_timer::Interval;

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

    fn throttle<'a>(self, millis: u64) -> Box<'a + Stream<Item = Self::Item, Error = Self::Error>>
    where
        Self: 'a,
    {
        let delay = time::Duration::from_millis(millis);
        Box::new(self.zip(
            Interval::new(time::Instant::now() + delay, delay).map_err(|_| unreachable!()),
        ).and_then(|(item, _)| Ok(item)))
    }
}

impl<T: Stream> StreamExt for T {}
