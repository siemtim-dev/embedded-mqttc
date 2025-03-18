use std::time::Duration;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use queue_vec::{split::WithQueuedVecInner, QueuedVec};


#[tokio::main]
async fn main () {

    let vec = QueuedVec::<CriticalSectionRawMutex, usize, 4>::new();
    vec.try_push(0).unwrap(); // Should succeed because vec is empty

    let slow_worker = async {
        let mut continue_work = true;
        while continue_work {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue_work = vec.operate(|data| {
                data.iter_mut().for_each(|element| *element += 1);
                data.retain(|element| *element < 10);

                ! data.is_empty()
            });
        }
    };

    // Producing new elements is only slowed by push().await
    let fast_producer = async {
        for i in 0..32 {
            vec.push(i % 4).await;
        }
    };

    tokio::join!(slow_worker, fast_producer);

}