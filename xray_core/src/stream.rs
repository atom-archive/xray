use futures::{Async, Poll, Stream};

pub struct Last<T: Stream>(T);

impl<T: Stream> Stream for Last<T> {
    type Item = T::Item;
    type Error = T::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut last_item;

        match self.0.poll()? {
            Async::NotReady => return Ok(Async::NotReady),
            Async::Ready(None) => return Ok(Async::Ready(None)),
            Async::Ready(i) => last_item = i,
        }

        loop {
            match self.0.poll()? {
                Async::NotReady => break,
                Async::Ready(i) => last_item = i
            }
        }

        Ok(Async::Ready(last_item))
    }
}
