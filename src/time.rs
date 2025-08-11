/// Calls to [`Instant::now()`] fail during tests. So this modul eis used to exchange the 
/// functions dynamicly

#[cfg(feature = "embassy")]
pub(crate) use embassy_time::{Duration, Instant};

#[cfg(feature = "std")]
pub(crate) use std::time::{Duration, Instant};

pub(crate) fn now() -> Instant {
    #[cfg(not(test))]
    return Instant::now();

    #[cfg(test)]
    return test_time::NOW.with(|test_time| test_time.now());
}

#[cfg(feature = "std")]
pub(crate) type SleepFuture = tokio::time::Sleep;

#[cfg(feature = "embassy")]
pub(crate) type SleepFuture = embassy_time::Timer;

#[cfg(feature = "embassy")]
fn sleep_intern(duration: Duration) -> SleepFuture {
    use embassy_time::Timer;

    Timer::after(duration)
}

#[cfg(feature = "std")]
fn sleep_intern(duration: Duration) -> SleepFuture {
    use tokio::time::sleep;

    sleep(duration)
}

pub(crate) async fn sleep(duration: Duration) {

    #[cfg(not(test))]
    sleep_intern(duration).await;

    #[cfg(test)]
    test_time::TestTimeFuture::new(duration).await;
}

#[cfg(test)]
pub(crate) mod test_time {
    extern crate std;

    use core::cell::RefCell;
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll};

    use embassy_sync::waitqueue::MultiWakerRegistration;
    use super::{now, Instant};
    use super::Duration;

    pub(super) struct TestTimeFuture{
        wait_until: Instant,
        wait_future: super::SleepFuture,
    }

    impl TestTimeFuture {
        pub(crate) fn new(wait_duration: Duration) -> Self {
            let now = now();
            let wait_until = now + wait_duration;
            Self {
                wait_until,
                wait_future: super::sleep_intern(wait_duration)
            }
        }
    }

    impl Future for TestTimeFuture {
        type Output = ();
    
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            NOW.with(|inner| {
                let mut inner = inner.0.borrow_mut();

                match inner.source {
                    TimeSource::Static(now) => {
                        if now >= self.wait_until {
                            Poll::Ready(())
                        } else {
                            inner.wakers.register(cx.waker());
                            Poll::Pending
                        }
                    },
                    TimeSource::Default => {
                        let pinned = unsafe {
                            self.map_unchecked_mut(|this| &mut this.wait_future)
                        };
                        pinned.poll(cx)
                    },
                }
            })
        }
    }

    struct TestTimeInner {
        source: TimeSource,
        wakers: MultiWakerRegistration<64>
    }

    impl TestTimeInner {
        const fn new() -> Self {
            Self {
                source: TimeSource::Default,
                wakers: MultiWakerRegistration::new()
            }
        }

        pub(crate) fn get_time_source<'a>(&'a self) -> &'a TimeSource {
            &self.source
        }

        pub(super) fn now(&self) -> super::Instant {
            match self.source {
                TimeSource::Static(instant) => instant.clone(),
                TimeSource::Default => Instant::now(),
            }
        }
    }

    pub struct TestTime(RefCell<TestTimeInner>);

    impl TestTime {
        const fn new() -> Self {
            Self(RefCell::new(TestTimeInner::new()))
        }

        pub(crate) fn get_time_source(&self) -> TimeSource {
            self.0.borrow().get_time_source().clone()
        }

        pub(super) fn now(&self) -> super::Instant {
            self.0.borrow().now()
        }
    }

    thread_local! {
        pub(super) static NOW: TestTime = TestTime::new();
    }


    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TimeSource {
        Static(Instant),
        Default
    }

    #[allow(dead_code)]
    pub(crate) fn set_time(new_now: Instant) {
        NOW.with(| inner |{
            let mut inner = inner.0.borrow_mut();
            inner.source = TimeSource::Static(new_now);
            inner.wakers.wake();
        })
    }

    #[allow(dead_code)]
    pub(crate) fn set_static_now() {
        let now = Instant::now();
        NOW.with(|inner| {
            let mut inner = inner.0.borrow_mut();
            inner.source = TimeSource::Static(now);
            inner.wakers.wake();
        })
    }

    /// Sets the time source for tests to the default for the environment
    #[allow(dead_code)]
    pub(crate) fn set_default() {
        NOW.with(|inner| {
            let mut inner = inner.0.borrow_mut();
            inner.source = TimeSource::Default;
        })
    }
    
    pub(crate) fn advance_time(duration: Duration) {
        NOW.with(|inner| {
            let mut inner = inner.0.borrow_mut();
            match &mut inner.source {
                TimeSource::Static(instant) => *instant += duration,
                TimeSource::Default => panic!("cannot advance time: TimeSource::Default"),
            }

            inner.wakers.wake();
        })
    }

}

#[cfg(test)]
mod tests {

    use crate::time::test_time::TimeSource;

    use super::Duration;

    #[tokio::test]
    async fn test_now() {
        super::test_time::set_default();

        super::test_time::NOW.with(|inner|{
            assert_eq!(
                inner.get_time_source(), 
                TimeSource::Default
            );
        });
        
        let now1 = super::now();

        tokio::time::sleep(core::time::Duration::from_millis(50)).await;

        let now2 = super::now();

        assert!(now2 > now1);
    }

    #[tokio::test]
    async fn test_now_static() {
        super::test_time::set_static_now();
        
        let now1 = super::now();

        tokio::time::sleep(core::time::Duration::from_millis(50)).await;

        let now2 = super::now();

        assert_eq!(now2, now1);
    }


    #[tokio::test]
    async fn test_wait_static() {
        super::test_time::set_static_now();
        
        let future = super::test_time::TestTimeFuture::new(Duration::from_secs(20));
        tokio::pin!(future);

        let wait = tokio::time::sleep(std::time::Duration::from_millis(50));
        tokio::pin!(wait);

        tokio::select! {
            _ = &mut future => {
                panic!("future must not complete before time is set");
            },
            _ = wait => {}
        }

        let wait = tokio::time::sleep(std::time::Duration::from_millis(50));
        tokio::pin!(wait);
        super::test_time::advance_time(Duration::from_secs(21));

        tokio::select! {
            _ = &mut future => {},
            _ = wait => {
                panic!("future must complete after time is set");
            }
        }

    }
}
