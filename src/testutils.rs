use core::{future::Future, pin::Pin, task::{Context, Poll}};

mod fake_context {
    use core::ptr;
    use core::task::{RawWaker, RawWakerVTable, Waker};

    // --- no-op RawWaker callbacks ---
    unsafe fn raw_clone(_: *const ()) -> RawWaker {
        // erzeugt einen neuen RawWaker mit der gleichen VTable
        RawWaker::new(ptr::null(), &VTABLE)
    }

    unsafe fn raw_wake(_: *const ()) {
        // intentionally does nothing
    }

    unsafe fn raw_wake_by_ref(_: *const ()) {
        // intentionally does nothing
    }

    unsafe fn raw_drop(_: *const ()) {
        // intentionally does nothing
    }

    // Static VTABLE mit den no-op-Funktionen.
    // RawWakerVTable::new ist const-frei (arbeitet zur Laufzeit), aber ein static ist hier möglich.
    static VTABLE: RawWakerVTable = RawWakerVTable::new(
        raw_clone,
        raw_wake,
        raw_wake_by_ref,
        raw_drop,
    );

    /// Erzeugt einen Waker, dessen Aktionen keine Effekte haben.
    /// Dieser Waker ist sicher in Tests und beim Polling, wenn
    /// du keine Wake-Mechanik benötigst.
    pub fn noop_waker() -> Waker {
        // RawWaker::new ist unsafe, Waker::from_raw ebenfalls.
        unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) }
    }
}



pub fn poll_once<F: Future<Output = T>, T>(f: Pin<&mut F>) -> Poll<T> {
    let waker = fake_context::noop_waker();
    let mut cx = Context::from_waker(&waker);
    f.poll(&mut cx)
}

pub fn assert_ready<F: Future<Output = T>, T>(f: Pin<&mut F>) -> T {
    let poll = poll_once(f);
    match poll {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("assert future is ready but returned Poll::Pending"),
    }
}

pub fn assert_ready_pin<F: Future<Output = T>, T>(f: F) -> T {
    let mut f = Box::pin(f);
    let poll = poll_once(f.as_mut());
    match poll {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("assert future is ready but returned Poll::Pending"),
    }
}

pub fn assert_pending<F: Future<Output = T>, T>(f: Pin<&mut F>) {
    let poll = poll_once(f);
    match poll {
        Poll::Ready(_result) => panic!("assert future is pending but resolved"),
        Poll::Pending => {},
    }
}

#[cfg(test)]
mod test {
    use core::{future::Future, pin::Pin, task::Poll};

    use crate::testutils::poll_once;

    struct NeverFuture;

    impl Future for NeverFuture {
        type Output = ();
    
        fn poll(self: Pin<&mut Self>, _cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    #[test]
    fn test_poll_once() {
        let mut f = NeverFuture;
        let f = unsafe { Pin::new_unchecked(&mut f) };
        assert_eq!(poll_once(f), Poll::Pending)
    }

}