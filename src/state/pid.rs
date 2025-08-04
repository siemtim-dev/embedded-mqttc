use core::cell::RefCell;

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use mqttrs2::Pid;



pub struct PidSource {
    counter: Mutex<CriticalSectionRawMutex, RefCell<Pid>>
}

impl PidSource {
    pub fn new() -> Self {
        Self {
            counter: Mutex::new(RefCell::new(Pid::default()))
        }
    }

    /// Genrates the next unique pid for the packet
    ///
    pub(crate) fn next_pid(&self) -> Pid {
        self.counter.lock(|pid|{
            let mut pid = pid.borrow_mut();

            let result = pid.clone();
            *pid = result + 1;
            result
        })
    }
}


#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;
    use std::sync::Arc;
    use std::thread;

    use mqttrs2::Pid;

    use super::PidSource;

    #[test]
    fn test_next_pid() {
        let pid_source = PidSource::new();
        let mut pids = Vec::new();

        for _ in 0..1000 {
            let pid = pid_source.next_pid();

            assert!( ! pids.iter().any(|el| *el == pid));

            pids.push(pid);
        }
    }

    #[test]
    fn test_concurrent_access() {
        let pid_source = Arc::new(PidSource::new());

        fn start(pid_source: Arc<PidSource>) -> thread::JoinHandle<Vec<Pid>> {
            thread::spawn(move || {
                let mut pids = Vec::new();
                for _ in 0..100 {
                    let pid = pid_source.next_pid();
        
                    assert!( ! pids.iter().any(|el| *el == pid));
        
                    pids.push(pid);
                }

                pids
            })  
        }

        let mut pids = Vec::new();
        let mut handles = Vec::new();

        for _ in 0..50 {
            handles.push(start(pid_source.clone()));
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
}