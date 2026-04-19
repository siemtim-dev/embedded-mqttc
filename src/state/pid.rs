use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use heapless::Deque;
use mqttrs2::Pid;

#[cfg(test)]
pub mod inspections {
    use core::cell::RefCell;

    use mqttrs2::Pid;


    extern crate std;

    thread_local! {
        pub static FREED_PIDS: RefCell<Vec<Pid>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn on_pid_freed(pid: Pid) {
        FREED_PIDS.with_borrow_mut(|freed| freed.push(pid));
    }

    pub fn assert_freed(pid: Pid) {
        let is_freed = FREED_PIDS.with_borrow(|freed| freed.contains(&pid));
        assert!(is_freed, "assert that {:?} is freed", pid)
    }

    pub fn assert_not_freed(pid: Pid) {
        let is_freed = FREED_PIDS.with_borrow(|freed| freed.contains(&pid));
        assert!(! is_freed, "assert that {:?} is freed", pid)
    }

}

const PID_POOL_SIZE: usize = 16;

static POOL: Mutex<CriticalSectionRawMutex, RefCell<PidPool<PID_POOL_SIZE>>> = Mutex::new(RefCell::new(PidPool::new()));

pub fn next_pid() -> Pid {
    POOL.lock(|inner| {
        let mut inner = inner.borrow_mut();
        inner.next_pid()
    })
}

pub fn free_pid(pid: Pid) {
    POOL.lock(|inner| {
        let mut inner = inner.borrow_mut();
        inner.free_pid(pid);
    });

    #[cfg(test)]
    inspections::on_pid_freed(pid);
}

struct PidPool <const POOL_SIZE: usize>{
    pool: Deque<u16, POOL_SIZE>,
    next_new_pid: u16
}

impl <const POOL_SIZE: usize> PidPool<POOL_SIZE> {

    const fn new() -> Self {
        Self {
            pool: Deque::new(),
            next_new_pid: 1
        }
    }

    fn next_pid(&mut self) -> Pid {
        if let Some(pid) = self.pool.pop_front() {
            trace!("next pid from pool {}", pid);
            pid.try_into().expect("unexpected 0 in pid pool")
        } else {
            self.take()
        }
    }

    fn take(&mut self) -> Pid {
        let pid = self.next_new_pid;
        if self.next_new_pid == u16::MAX {
            panic!("used complete pid pool!");
        } else {
            self.next_new_pid += 1;
        }
        trace!("take new pid {}", pid);
        pid.try_into().expect("counter wrong: should start at 1")
    }

    fn free_pid(&mut self, pid: Pid) {
        trace!("free pid {}", pid);
        match self.pool.push_back(pid.into()) {
            Ok(()) => {},
            Err(pid) => {
                warn!("pool too small to hand back pid {}: lost forever", pid);
            }
        }
    }
}






#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;
    use std::thread;

    use mqttrs2::Pid;

    use crate::state::pid::PidPool;

    #[test]
    fn test_next_pid() {
        let mut pids = Vec::new();

        for _ in 0..1000 {
            let pid = super::next_pid();

            assert!( ! pids.iter().any(|el| *el == pid));

            pids.push(pid);
        }
    }

    #[test]
    fn test_concurrent_access() {

        fn start() -> thread::JoinHandle<Vec<Pid>> {
            thread::spawn(move || {
                let mut pids = Vec::new();
                for _ in 0..100 {
                    let pid = super::next_pid();
        
                    assert!( ! pids.iter().any(|el| *el == pid));
        
                    pids.push(pid);
                }

                pids
            })  
        }

        let mut pids = Vec::new();
        let mut handles = Vec::new();

        for _ in 0..50 {
            handles.push(start());
        }

        for h in handles {
            let mut v = h.join().unwrap();
            pids.append(&mut v);
        }

        for pid in &pids {
            let n = pids.iter().filter(|el| **el == *pid).count();
            assert_eq!(n, 1, "Pid {} was present {} times", pid.get(), n);
        }

    }

    #[test]
    fn test_pid_reuse () {

        let mut pool = PidPool::<16>::new();

        let pid1 = pool.next_pid();
        let pid2 = pool.next_pid();

        assert!(pid1 != pid2);

        pool.free_pid(pid1);
        let pid3 = pool.next_pid();
        assert_eq!(pid1, pid3); 
    }
}