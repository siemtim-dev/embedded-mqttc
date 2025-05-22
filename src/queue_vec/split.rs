
use core::future::Future;
use core::marker::PhantomData;
use core::{pin::Pin, task::{Context, Poll}};

use embassy_sync::waitqueue::MultiWakerRegistration;
use heapless::Vec;

use super::MAX_WAKERS;

pub trait WithQueuedVecInner<A: 'static, T: 'static, const N: usize> {
    fn with_queued_vec_inner<F, O>(&self, operation: F) -> O where F: FnOnce(&mut QueuedVecInner<A, T, N>) -> O;

    /// Pushes an item to the vec. Waits until there is space.
    fn push<'a>(&'a self, item: T) -> PushFuture<'a, Self, A, T, N> {
        PushFuture::new(self, item)
    }

    fn try_push(&self, item: T) -> Result<(), T> {
        self.with_queued_vec_inner(|inner| inner.try_push(item))
    }

    /// Perfroms an operation synchronously on the contained elements and returns the result.
    fn operate<F, O>(&self, operation: F) -> O 
        where F: FnOnce(&mut Vec<T, N>) -> O {

        self.with_queued_vec_inner(|inner|{
            let result = operation(&mut inner.data);
            if ! inner.data.is_full() {
                inner.wakers.wake();
            }
            result
        })
    }

    /// Retains only the elemnts matching [`f`]
    fn retain<F>(&self, f: F) where F: FnMut(&T) -> bool{
        self.operate(|data| {
            data.retain(f);
        })
    }
}

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PushFuture<'a, I: WithQueuedVecInner<A, T, N> + ?Sized, A: 'static, T: 'static, const N: usize> {
    queue: &'a I,
    item: Option<T>,
    _phantom_data: PhantomData<A>
}

impl <'a, I: WithQueuedVecInner<A, T, N> + ?Sized, A: 'static, T: 'static, const N: usize> PushFuture<'a, I, A, T, N> {
    fn new(queue: &'a I, item: T) -> Self {
        Self {
            queue,
            item: Some(item),
            _phantom_data: PhantomData
        }
    }
}

impl <'a, I: WithQueuedVecInner<A, T, N> + ?Sized, A: 'static, T: 'static, const N: usize> Unpin for PushFuture<'a, I, A, T, N> {}

impl <'a, I: WithQueuedVecInner<A, T, N> + ?Sized, A: 'static, T: 'static, const N: usize> Future for PushFuture<'a, I, A, T, N> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.queue.with_queued_vec_inner(|inner|{
            inner.poll_push(&mut self.item, cx)
        })
    }
}

pub struct WorkingCopy<'a, T, const N: usize> {
    pub data: &'a mut Vec<T, N>,
    wakers: &'a mut MultiWakerRegistration<MAX_WAKERS>,
}

impl <'a, T, const N: usize> Drop for WorkingCopy<'a, T, N> {
    fn drop(&mut self) {
        if ! self.data.is_full() {
            self.wakers.wake();
        }
    }
}

pub struct QueuedVecInner<A: 'static, T: 'static, const N: usize> {
    wakers: MultiWakerRegistration<MAX_WAKERS>,
    data: Vec<T, N>,
    additional_data: A

}

impl <A, T: 'static, const N: usize> QueuedVecInner<A, T, N> {
    pub fn new(additional_data: A) -> Self {
        Self {
            wakers: MultiWakerRegistration::new(),
            data: Vec::new(),
            additional_data
        }
    }

    pub fn working_copy<'a>(&'a mut self) -> ( WorkingCopy<'a, T, N>, &'a mut A ){
        (WorkingCopy { data: &mut self.data, wakers: &mut self.wakers }, &mut self.additional_data)
    }

    pub fn poll_push(&mut self, item: &mut Option<T>, cx: &mut Context<'_>) -> Poll<()>{
        if self.data.is_full() {
            self.wakers.register(cx.waker());
            Poll::Pending
        } else {
            let item = item.take()
                .ok_or("Illegal State: poll() called but item to add is not present")
                .unwrap();
            
            self.data.push(item)
                .map_err(|_| "Err: checkt if data is bull, but push failed").unwrap();

            Poll::Ready(())
        }
    }

    pub fn try_push(&mut self, item: T) -> Result<(), T> {
        self.data.push(item)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    
    use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
    use tokio::time::sleep;
    use core::{cell::RefCell, time::Duration};
    use std::sync::Arc;

    use super::{QueuedVecInner, WithQueuedVecInner};

    struct TestQueuedVec <A: 'static, T: 'static, const N: usize> {
        inner: Mutex<CriticalSectionRawMutex, RefCell<QueuedVecInner<A, T, N>>>
    }

    impl <A: 'static, T: 'static, const N: usize> TestQueuedVec <A, T, N> {
        fn new(additional_data: A) -> Self {
            Self {
                inner: Mutex::new(RefCell::new(QueuedVecInner::new(additional_data)))
            }
        }
    }

    impl <A: 'static, T: 'static, const N: usize> WithQueuedVecInner<A, T, N> for TestQueuedVec <A, T, N> {
        fn with_queued_vec_inner<F, O>(&self, operation: F) -> O where F: FnOnce(&mut QueuedVecInner<A, T, N>) -> O {
            self.inner.lock(|inner| {
                let mut inner = inner.borrow_mut();
                operation(&mut inner)
            })
        }
    }



    #[tokio::test]
    async fn test_add() {
        // let executor = ThreadPool::new().unwrap();

        let q = TestQueuedVec::<(), usize, 4>::new(());

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

        let q = Arc::new(TestQueuedVec::<(), usize, 4>::new(()));
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

        let q = Arc::new(TestQueuedVec::<(), usize, 4>::new(()));

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