use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use fastrand::FastRand;
use kasync2::executor::{Executor, Worker};

async fn work() -> usize {
    let val = 1 + 1;
    kasync2::task::yield_now().await;
    black_box(val)
}

fn single_threaded_spawn(c: &mut Criterion) {
    static EXEC: Executor = Executor::new();
    let mut worker = Worker::new(&EXEC, FastRand::from_seed(0));

    c.bench_function("single_threaded_spawn", |b| {
        b.iter(|| {
            kasync2::test_util::block_on(worker.run(async {
                let h = EXEC.try_spawn(work()).unwrap();
                assert_eq!(h.await.unwrap(), 2);
            }))
        })
    });
}

fn single_threaded_spawn10(c: &mut Criterion) {
    static EXEC: Executor = Executor::new();
    let mut worker = Worker::new(&EXEC, FastRand::from_seed(0));

    c.bench_function("single_threaded_spawn10", |b| {
        b.iter(|| {
            kasync2::test_util::block_on(worker.run(async {
                let mut handles = Vec::with_capacity(10);
                for _ in 0..10 {
                    let h = EXEC.build_task().try_spawn(work()).unwrap();
                    handles.push(h);
                }

                for handle in handles {
                    assert_eq!(handle.await.unwrap(), 2);
                }
            }))
            .unwrap();
        })
    });
}

// fn multi_threaded_spawn(c: &mut Criterion) {
//     static EXEC: Executor<StdPark> = new_executor!(1);
//
//     let h = std::thread::spawn(|| {
//         let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
//         worker.run();
//     });
//
//     let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
//     c.bench_function("multi_threaded_spawn", |b| {
//         b.iter(|| {
//             worker.block_on(async {
//                 let (task, h) = EXEC.task_builder().try_build(work()).unwrap();
//                 EXEC.spawn_allocated(task);
//                 assert_eq!(h.await.unwrap(), 2);
//             });
//         })
//     });
//
//     EXEC.stop();
//     h.join().unwrap();
// }
//
// fn multi_threaded_spawn10(c: &mut Criterion) {
//     static EXEC: Executor<StdPark> = new_executor!(1);
//     let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
//
//     let h = std::thread::spawn(|| {
//         let mut worker = Worker::new(&EXEC, 0, StdPark::for_current(), FastRand::from_seed(0));
//         worker.run();
//     });
//
//     c.bench_function("multi_threaded_spawn10", |b| {
//         b.iter(|| {
//             worker.block_on(async {
//                 let mut handles = Vec::with_capacity(10);
//                 for _ in 0..10 {
//                     let (task, h) = EXEC.task_builder().try_build(work()).unwrap();
//                     handles.push(h);
//                     EXEC.spawn_allocated(task);
//                 }
//
//                 for handle in handles {
//                     assert_eq!(handle.await.unwrap(), 2);
//                 }
//             });
//         })
//     });
//
//     EXEC.stop();
//     h.join().unwrap();
// }

criterion_group!(
    spawn,
    single_threaded_spawn,
    // single_threaded_spawn10,
    // multi_threaded_spawn,
    // multi_threaded_spawn10,
);

criterion_main!(spawn);
