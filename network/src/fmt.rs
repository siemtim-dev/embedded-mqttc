#![macro_use]

use core::fmt::Debug;

#[allow(dead_code)]
pub(crate) struct Debug2Format<T: Debug>(pub(crate) T);

#[cfg(feature = "tracing")]
impl <T: Debug> core::fmt::Display for crate::fmt::Debug2Format<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(feature = "defmt")]
impl <T: Debug> defmt::Format for crate::fmt::Debug2Format<T> {
    fn format(&self, fmt: defmt::Formatter) {
        let d = defmt::Debug2Format(&self.0);
        d.format(fmt);
    }
}

impl <T: Debug> Debug for Debug2Format<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

#[collapse_debuginfo(yes)]
macro_rules! trace {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::trace!($s $(, $x)*);
        #[cfg(feature = "tracing")]
        ::tracing::trace!($s $(, crate::fmt::Debug2Format($x))*);
        #[cfg(not(any(feature="defmt", feature = "tracing")))]
        let _ = ($( & $x ),*);
    };
}

/* 
#[collapse_debuginfo(yes)]
macro_rules! debug {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::debug!($s $(, $x)*);
        #[cfg(feature = "tracing")]
        ::tracing::debug!($s $(, crate::fmt::Debug2Format($x))*);
        #[cfg(not(any(feature="defmt", feature = "tracing")))]
        let _ = ($( & $x ),*);
    };
}
*/


#[collapse_debuginfo(yes)]
macro_rules! info {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::info!($s $(, $x)*);
        #[cfg(feature = "tracing")]
        ::tracing::info!($s $(, crate::fmt::Debug2Format($x))*);
        #[cfg(not(any(feature="defmt", feature = "tracing")))]
        let _ = ($( & $x ),*);
    };
}


#[collapse_debuginfo(yes)]
macro_rules! warn {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::warn!($s $(, $x)*);
        #[cfg(feature = "tracing")]
        ::tracing::warn!($s $(, crate::fmt::Debug2Format($x))*);
        #[cfg(not(any(feature="defmt", feature = "tracing")))]
        let _ = ($( & $x ),*);
    };
}

#[collapse_debuginfo(yes)]
macro_rules! error {
    ($s:literal $(, $x:expr)* $(,)?) => {
        #[cfg(feature = "defmt")]
        ::defmt::error!($s $(, $x)*);
        #[cfg(feature = "tracing")]
        ::tracing::error!($s $(, crate::fmt::Debug2Format($x))*);
        #[cfg(not(any(feature="defmt", feature = "tracing")))]
        let _ = ($( & $x ),*);
    };
}