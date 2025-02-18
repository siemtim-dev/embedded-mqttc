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
    return test_time::NOW.now();
}

#[cfg(feature = "embassy")]
pub(crate) async fn sleep(duration: Duration) {
    use embassy_time::Timer;

    Timer::after(duration).await;
}

#[cfg(feature = "std")]
pub(crate) async fn sleep(duration: Duration) {
    use tokio::time::sleep;

    sleep(duration).await;
}

#[cfg(test)]
pub(crate) mod test_time {
    extern crate std;

    use core::{cell::RefCell, ops::DerefMut};

    use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
    use super::Instant;
    use super::Duration;

    pub struct TestTime(Mutex<CriticalSectionRawMutex, RefCell<TimeSource>>);



    impl TestTime {
        const fn new() -> Self {
            Self(Mutex::new(RefCell::new(TimeSource::Default)))
        }

        pub(super) fn now(&self) -> super::Instant {
            self.0.lock(|inner| {
                let inner = inner.borrow();

                match *inner {
                    TimeSource::Static(instant) => instant.clone(),
                    TimeSource::Default => Instant::now(),
                }
            })
        }
    }

    pub(super) static NOW: TestTime = TestTime::new();


    pub enum TimeSource {
        Static(Instant),
        Default
    }

    #[allow(dead_code)]
    #[cfg(test)]
    pub(crate) fn set_time(new_now: Instant) {
        NOW.0.lock(|inner| {
            let mut inner = inner.borrow_mut();
            *inner = TimeSource::Static(new_now);
        })
    }

    #[allow(dead_code)]
    #[cfg(test)]
    pub(crate) fn set_default() {
        NOW.0.lock(|inner| {
            let mut inner = inner.borrow_mut();
            *inner = TimeSource::Default;
        })
    }
    
    #[cfg(test)]
    pub(crate) fn advance_time(duration: Duration) {
        NOW.0.lock(|inner| {
            let mut inner = inner.borrow_mut();
            match inner.deref_mut() {
                TimeSource::Static(instant) => *instant += duration,
                TimeSource::Default => panic!("cannot advance time: TimeSource::Default"),
            }
        })
    }

}
