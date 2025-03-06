
#![cfg_attr(not(feature = "std"), no_std)]

use core::cell::RefCell;
use core::future::Future;
use core::{pin::Pin, task::{Context, Poll}};

use embassy_sync::{blocking_mutex::{raw::RawMutex, Mutex}, waitqueue::MultiWakerRegistration};
use heapless::Vec;

pub const MAX_WAKERS: usize = 4;

pub struct QueuedVec<R: RawMutex, T: 'static, const N: usize> {
    inner: Mutex<R, RefCell<Inner<T, N>>>
}

impl <R: RawMutex, T: 'static, const N: usize> QueuedVec<R, T, N> {

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(Inner::new()))
        }
    }

    /// Perfroms an operation synchronously on the contained elements and returns the result.
    pub fn operate<F, O>(&self, operation: F) -> O 
        where F: FnOnce(&mut Vec<T, N>) -> O {

        self.inner.lock(|inner|{
            let mut inner = inner.borrow_mut();
            let result = operation(&mut inner.data);
            if ! inner.data.is_full() {
                inner.wakers.wake();
            }
            result
        })
    }

    /// Pushes an item to the vec. Waits until there is space.
    pub fn push<'a>(&'a self, item: T) -> PushFuture<'a, R, T, N> {
        PushFuture::new(self, item)
    }

    /// Retains only the elemnts matching [`f`]
    pub fn retain<F>(&self, f: F) where F: FnMut(&T) -> bool{
        self.operate(|data| {
            data.retain(f);
        })
    }

    /// Remove all elements from the queue which satisfy the remove_where function.
    /// Every call to next on the returned iterator removes one element and returns it if present
    pub fn remove<'a, F: FnMut(&T) -> bool>(&'a self, remove_where: F) -> RemoveIterator<'a, R, T, F, N> {
        RemoveIterator{
            q: self,
            remove_where
        }
    }

}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PushFuture<'a, R: RawMutex, T: 'static, const N: usize> {
    queue: &'a QueuedVec<R, T, N>,
    item: Option<T>
}

impl <'a, R: RawMutex, T, const N: usize> PushFuture<'a, R, T, N> {
    fn new(queue: &'a QueuedVec<R, T, N>, item: T) -> Self {
        Self {
            queue,
            item: Some(item)
        }
    }
}

impl <'a, R: RawMutex, T: 'static, const N: usize> Unpin for PushFuture<'a, R, T, N> {}

impl <'a, R: RawMutex, T: 'static, const N: usize> Future for PushFuture<'a, R, T, N> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.queue.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            if inner.data.is_full() {
                inner.wakers.register(cx.waker());
                Poll::Pending
            } else {
                let item = self.item.take()
                    .ok_or("Illegal State: poll() called but item to add is not present")
                    .unwrap();
                inner.data.push(item)
                    .map_err(|_| "Err: checkt if data is bull, but push failed").unwrap();

                Poll::Ready(())
            }
        })
    }
}

struct Inner<T: 'static, const N: usize> {
    wakers: MultiWakerRegistration<MAX_WAKERS>,

    data: Vec<T, N>

}

impl <T: 'static, const N: usize> Inner<T, N> {
    pub fn new() -> Self {
        Self {
            wakers: MultiWakerRegistration::new(),
            data: Vec::new()
        }
    }
}

pub struct RemoveIterator <'a, R: RawMutex, T: 'static, F: FnMut(&T) -> bool, const N: usize>{
    q: &'a QueuedVec<R, T, N>,
    remove_where: F
}

impl <'a, R: RawMutex, T: 'static, F: FnMut(&T) -> bool, const N: usize> Iterator for RemoveIterator<'a, R, T, F, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.q.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            for i in 0..inner.data.len() {
                if (self.remove_where)(&inner.data[i]) {
                    return Some(inner.data.remove(i))
                }
            }
            None
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use tokio::time::sleep;
    use core::time::Duration;
    use std::sync::Arc;

    use crate::QueuedVec;

    #[tokio::test]
    async fn test_add() {
        // let executor = ThreadPool::new().unwrap();

        let q = QueuedVec::<CriticalSectionRawMutex, usize, 4>::new();

        q.push(1).await;
        q.push(2).await;
        q.push(3).await;
        q.push(4).await;

        q.operate(|v| {
            assert_eq!(&v[..], &[1, 2, 3, 4]);
        });
    }

    #[tokio::test]
    async fn test_wait_add() {

        let q = Arc::new(QueuedVec::<CriticalSectionRawMutex, usize, 4>::new());
        let q2 = q.clone();
        
        q.push(1).await;
        q.push(2).await;
        q.push(3).await;
        q.push(4).await;

        tokio::spawn(async move {
            q2.push(5).await;
        });

        sleep(Duration::from_millis(15)).await;

        q.operate(|v|{
            assert_eq!(&v[..], &[1, 2, 3, 4]);
            v.remove(0);
        });

        sleep(Duration::from_millis(15)).await;
        
        q.operate(|v| {
            assert_eq!(&v[..], &[2, 3, 4, 5]);
        });
    }

    #[tokio::test]
    async fn test_parallelism() {

        const EXPECTED: usize = 190;

        let q = Arc::new(QueuedVec::<CriticalSectionRawMutex, usize, 4>::new());

        let q1 = q.clone();
        let jh1 = tokio::spawn(async move {
            for i in 0..10 {
                q1.push(i * 2).await;
            }
        });

        let q2 = q.clone();
        let jh2 = tokio::spawn(async move {
            for i in 0..10 {
                q2.push(i * 2 + 1).await;
            }
        });

        let test_future = async {
            sleep(Duration::from_millis(15)).await;

            let mut n = 0;

            while q.operate(|v| {
                match v.pop() {
                    Some(value) => {
                        n += value;
                        true
                    },
                        None => false,
                    }
                }) {
                    sleep(Duration::from_millis(5)).await;
                }

            assert_eq!(n, EXPECTED);
        };

        let (_, r2, r3) = tokio::join!(test_future, jh1, jh2);
        r2.unwrap();
        r3.unwrap();

    }

}