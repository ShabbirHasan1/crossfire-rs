use crate::locked_waker::LockedWaker;
use crate::{AsyncRx, TryRecvError};
use futures::stream;
use std::fmt;
use std::pin::Pin;
use std::task::*;

/// Constructed by [AsyncRx::into_stream()](crate::AsyncRx::into_stream())
///
/// Implemented futures::stream::Stream;
pub struct AsyncStream<T> {
    rx: AsyncRx<T>,
    waker: Option<LockedWaker>,
    ended: bool,
}

impl<T> fmt::Debug for AsyncStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AsyncStream")
    }
}

impl<T> fmt::Display for AsyncStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "AsyncStream")
    }
}

impl<T> AsyncStream<T>
where
    T: Unpin + Send + 'static,
{
    #[inline(always)]
    pub fn new(rx: AsyncRx<T>) -> Self {
        Self { rx, waker: None, ended: false }
    }

    /// Try to receive message, non-blocking.
    ///
    /// Returns `Ok(T)` on successful.
    ///
    /// Returns Err([TryRecvError::Empty]) when channel is empty.
    ///
    /// Returns Err([TryRecvError::Disconnected]) when all Tx dropped and channel is empty.
    #[inline]
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.rx.try_recv()
    }

    /// Probe possible messages in the channel (not accurate)
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    /// Whether there's message in the channel (not accurate)
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }

    /// poll_item() will try to receive message, if not successful, will register notification for
    /// the next poll.
    ///
    /// Returns `Ok(T)` on successful.
    ///
    /// Return Err([TryRecvError::Empty]) for Poll::Pending case.
    ///
    /// Return Err([TryRecvError::Disconnected]) when all Tx dropped and channel is empty.
    #[inline]
    pub fn poll_item(&mut self, ctx: &mut Context) -> Poll<Option<T>> {
        if self.ended {
            return Poll::Ready(None);
        }
        match self.rx.poll_item(ctx, &mut self.waker) {
            Ok(item) => Poll::Ready(Some(item)),
            Err(e) => {
                if e.is_empty() {
                    return Poll::Pending;
                }
                self.ended = true;
                return Poll::Ready(None);
            }
        }
    }
}

impl<T> stream::Stream for AsyncStream<T>
where
    T: Unpin + Send + 'static,
{
    type Item = T;

    #[inline]
    fn poll_next(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Option<Self::Item>> {
        let mut _self = self.get_mut();
        match _self.rx.poll_item(ctx, &mut _self.waker) {
            Err(e) => {
                if e.is_empty() {
                    return Poll::Pending;
                }
                _self.ended = true;
                return Poll::Ready(None);
            }
            Ok(item) => Poll::Ready(Some(item)),
        }
    }
}

impl<T> stream::FusedStream for AsyncStream<T>
where
    T: Unpin + Send + 'static,
{
    fn is_terminated(&self) -> bool {
        self.ended
    }
}

impl<T> Drop for AsyncStream<T> {
    fn drop(&mut self) {
        if let Some(waker) = self.waker.take() {
            self.rx.shared.clear_recv_wakers(waker.get_seq());
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::*;
    use futures::stream::{FusedStream, StreamExt};

    #[test]
    fn test_into_stream() {
        println!();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let total_message = 100;
            let (tx, rx) = crate::mpmc::bounded_async::<i32>(2);
            tokio::spawn(async move {
                println!("sender thread send {} message start", total_message);
                for i in 0i32..total_message {
                    let _ = tx.send(i).await;
                    // println!("send {}", i);
                }
                println!("sender thread send {} message end", total_message);
            });
            let mut s = rx.into_stream();

            for _i in 0..total_message {
                assert_eq!(s.next().await, Some(_i));
            }
            assert_eq!(s.next().await, None);
            assert!(s.is_terminated())
        });
    }
}
