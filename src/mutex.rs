use core::cell::UnsafeCell;
use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::waitqueue::MultiWakerRegistration;

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;

struct MutexInner<const WAKERS: usize> {
    wakers: MultiWakerRegistration<WAKERS>,
    locked: bool
}

impl<const WAKERS: usize> MutexInner<WAKERS> {
    fn new() -> Self {
        Self {
            wakers: MultiWakerRegistration::new(),
            locked: false
        }
    }
}

pub struct Mutex<M: RawMutex, T, const WAKERS: usize> {
    value: UnsafeCell<T>,
    inner: BlockingMutex<M, UnsafeCell<MutexInner<WAKERS>>>
}

impl<M: RawMutex, T, const WAKERS: usize> Mutex<M, T, WAKERS> {

    pub fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            inner: BlockingMutex::new(UnsafeCell::new(MutexInner::new()))
        }
    }

    // pub fn as_mut(&mut self) -> &mut T {
    //     self.value.get_mut()
    // }

    // pub fn into_inner(self) -> T {
    //     self.value.into_inner()
    // }

    #[cfg(test)]
    pub fn try_lock<'a>(&'a self) -> Result<Lock<'a, M, T, WAKERS>, ()> {
        self.inner.lock(|inner| unsafe {
            let inner = inner.get();
            let inner = &mut(*inner);

            if inner.locked {
                Err(())
            } else {
                inner.locked = true;
                Ok(Lock { 
                    mutex: self, 
                    value: self.value.get()
                })
            }
        })
    }

    pub fn lock(&self) -> LockFuture<'_, M, T, WAKERS> {
        LockFuture { mutex: self }
    }

    pub fn try_with_lock<'a, F, E>(&'a self, operation: F) -> TryWithLockFuture<'a, M, T, WAKERS, F, E> where F: Fn(&mut T) -> Result<bool, E> {
        TryWithLockFuture {
            mutex: self,
            operation
        }
    }

    fn poll_lock<'a>(&'a self, cx: &mut Context<'_>) -> Poll<Lock<'a, M, T, WAKERS>> {
        self.inner.lock(|inner| unsafe {
            let inner = inner.get();
            let inner = &mut(*inner);

            if inner.locked {
                inner.wakers.register(cx.waker());
                Poll::Pending
            } else {
                inner.locked = true;
                Poll::Ready(Lock { 
                    mutex: self, 
                    value: self.value.get()
                })
            }
        })
    }

    fn poll_try_with_lock<F, E>(&self, f: &F, cx: &mut Context<'_>) -> Poll<Result<(), E>> 
    where F: Fn(&mut T) -> Result<bool, E> {
        self.inner.lock(|inner| unsafe {
            let inner = inner.get();
            let inner = &mut(*inner);

            if inner.locked {
                inner.wakers.register(cx.waker());
                return Poll::Pending;
            }

            let value = self.value.get();
            let value = &mut (*value);

            match f(value) {
                Ok(true) => Poll::Ready(Ok(())),
                Ok(false) => {
                    inner.wakers.register(cx.waker());
                    Poll::Pending
                }
                Err(err) => Poll::Ready(Err(err))
            }
        })
    }

}

pub struct Lock<'a, M: RawMutex, T, const WAKERS: usize> {
    mutex: &'a Mutex<M, T, WAKERS>,
    value: *mut T
}

impl<'a, M: RawMutex, T, const WAKERS: usize> DerefMut for Lock<'a, M, T, WAKERS> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            &mut (*self.value)
        }
    }
}

impl<'a, M: RawMutex, T, const WAKERS: usize> Deref for Lock<'a, M, T, WAKERS> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &(*self.value)
        }
    }
}

impl<'a, M: RawMutex, T, const WAKERS: usize> Drop for Lock<'a, M, T, WAKERS> {
    fn drop(&mut self) {
        self.mutex.inner.lock(|inner| unsafe {
            let inner = inner.get();
            let inner = &mut (*inner);
            inner.locked = false;
            inner.wakers.wake();
        })
    }
}

pub struct  LockFuture <'a, M: RawMutex, T, const WAKERS: usize> {
    mutex: &'a Mutex<M, T, WAKERS>
}

impl<'a, M: RawMutex, T, const WAKERS: usize> Future for LockFuture<'a, M, T, WAKERS> {
    type Output = Lock<'a, M, T, WAKERS>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.mutex.poll_lock(cx)
    }
}

pub struct TryWithLockFuture <'a, M: RawMutex, T, const WAKERS: usize, F: Fn(&mut T) -> Result<bool, E>, E>{
    mutex: &'a Mutex<M, T, WAKERS>,
    operation: F
}

impl<'a, M: RawMutex, T, const WAKERS: usize, F: Fn(&mut T) -> Result<bool, E>, E> Future for TryWithLockFuture<'a, M, T, WAKERS, F, E> {
    type Output = Result<(), E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.mutex.poll_try_with_lock(&self.operation, cx)
    }
}



