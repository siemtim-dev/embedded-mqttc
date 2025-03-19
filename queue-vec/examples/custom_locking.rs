/// This example shows how to use the QueuedVecInner to implement custom locking


use std::{sync::Mutex, time::Duration};

use queue_vec::split::{QueuedVecInner, WithQueuedVecInner};

struct CustomLocking {
    inner: Mutex<QueuedVecInner<ProcessingData, usize, 4>>
}

impl WithQueuedVecInner<ProcessingData, usize, 4> for CustomLocking {
    fn with_queued_vec_inner<F, O>(&self, operation: F) -> O where F: FnOnce(&mut QueuedVecInner<ProcessingData, usize, 4>) -> O {
        let mut lock = self.inner.lock().unwrap();
        operation(&mut lock)
    }
}

struct ProcessingData {
    nines_produced: usize
}


#[tokio::main]
async fn main () {

    let vec = CustomLocking{
        inner: Mutex::new(QueuedVecInner::new(ProcessingData {
            nines_produced: 0
        }))
    };

    vec.try_push(1).unwrap(); // Should succeed because vec is empty

    // A slow worker processes elements
    let slow_worker_with_additional_data = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;

            {
                let mut lock = vec.inner.lock().unwrap();
                let (vec, processing_data) = lock.working_copy();
                
                vec.data.iter_mut().for_each(|element| {
                    if *element == 0 {
                        *element = 1;
                    } else {
                        *element *= 3;
                    }
                });

                let nines = vec.data.iter()
                    .filter(|element| **element == 9)
                    .count();

                processing_data.nines_produced += nines;

                println!("WORK: found {} nines this time, total nines: {}", nines, processing_data.nines_produced);

                vec.data.retain(|element| *element < 30 && *element != 9);

                if vec.data.is_empty() {
                    println!("WORK: vec is empty, finished working");
                    break;
                }
            }
        }
    };

    // Producing new elements is only slowed by push().await
    let fast_producer = async {
        for i in 0..32 {
            let number = i % 4 + 1;
            vec.push(number).await;
            println!("PUSH: added {}", number);
        }
        println!("PUSH: finished adding data");
    };

    tokio::join!(slow_worker_with_additional_data, fast_producer);

}