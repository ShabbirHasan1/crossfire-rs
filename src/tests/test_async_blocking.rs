use super::common::*;
use crate::*;
use captains_log::{logfn, *};
use rstest::*;
use std::thread;
use std::time::Duration;

#[fixture]
fn setup_log() {
    let _ = recipe::env_logger("LOG_FILE", "LOG_LEVEL").build().expect("log setup");
    //    let _ = recipe::ring_file("/tmp/ring.log", 512*1024*1024, Level::Debug, signal_consts::SIGHUP).build().expect("log_setup");
}

#[logfn]
#[rstest]
#[case(spsc::bounded_tx_async_rx_blocking(1))]
#[case(mpsc::bounded_tx_async_rx_blocking(1))]
#[case(mpmc::bounded_tx_async_rx_blocking(1))]
fn test_basic_bounded_empty_full_drop_rx<T: AsyncTxTrait<usize>, R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (T, R),
) {
    let (tx, rx) = channel;
    assert!(tx.is_empty());
    assert!(rx.is_empty());
    tx.try_send(1).expect("Ok");
    assert!(tx.is_full());
    assert!(rx.is_full());
    assert!(!tx.is_empty());
    assert_eq!(tx.is_disconnected(), false);
    assert_eq!(rx.is_disconnected(), false);
    drop(rx);
    assert_eq!(tx.is_disconnected(), true);
    assert_eq!(tx.as_ref().get_rx_count(), 0);
    assert_eq!(tx.as_ref().get_tx_count(), 1);
}

#[logfn]
#[rstest]
#[case(spsc::bounded_tx_async_rx_blocking(1))]
#[case(mpsc::bounded_tx_async_rx_blocking(1))]
#[case(mpmc::bounded_tx_async_rx_blocking(1))]
fn test_basic_bounded_empty_full_drop_tx<T: AsyncTxTrait<usize>, R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (T, R),
) {
    let (tx, rx) = channel;
    assert!(tx.is_empty());
    assert!(rx.is_empty());
    tx.try_send(1).expect("Ok");
    assert!(tx.is_full());
    assert!(rx.is_full());
    assert!(!tx.is_empty());
    assert_eq!(tx.is_disconnected(), false);
    assert_eq!(rx.is_disconnected(), false);
    drop(tx);
    assert_eq!(rx.is_disconnected(), true);
    assert_eq!(rx.as_ref().get_tx_count(), 0);
    assert_eq!(rx.as_ref().get_rx_count(), 1);
}

#[logfn]
#[rstest]
fn test_basic_compile_bounded_empty_full() {
    let (tx, rx) = mpmc::bounded_tx_async_rx_blocking::<usize>(1);
    assert!(tx.is_empty());
    assert!(rx.is_empty());
    tx.try_send(1).expect("ok");
    assert!(tx.is_full());
    assert!(!tx.is_empty());
    assert!(rx.is_full());
    assert_eq!(tx.get_tx_count(), 1);
    assert_eq!(rx.get_tx_count(), 1);
    assert_eq!(tx.is_disconnected(), false);
    assert_eq!(rx.is_disconnected(), false);
    drop(rx);
    assert_eq!(tx.is_disconnected(), true);
}

#[logfn]
#[rstest]
#[case(spsc::bounded_tx_async_rx_blocking::<usize>(100))]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(100))]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(100))]
fn test_basic_1_tx_async_1_rx_blocking<T: AsyncTxTrait<usize>, R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (T, R),
) {
    let (tx, rx) = channel;
    let rx_res = rx.try_recv();
    assert!(rx_res.is_err());
    assert!(rx_res.unwrap_err().is_empty());
    let batch_1: usize = 100;
    let batch_2: usize = 200;
    let th = thread::spawn(move || {
        for _ in 0..(batch_1 + batch_2) {
            match rx.recv() {
                Ok(i) => {
                    debug!("recv {}", i);
                }
                Err(e) => {
                    panic!("error {}", e);
                }
            }
        }
        let res = rx.recv();
        assert!(res.is_err());
    });

    runtime_block_on!(async move {
        for i in 0..batch_1 {
            let tx_res = tx.send(i).await;
            assert!(tx_res.is_ok());
        }
        for i in batch_1..(batch_1 + batch_2) {
            assert!(tx.send(10 + i).await.is_ok());
            sleep(Duration::from_millis(2)).await;
        }
    });
    let _ = th.join().unwrap();
}

#[logfn]
#[rstest]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 5)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 8)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 100)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 1000)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 5)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 8)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 100)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 1000)]
fn test_basic_multi_tx_async_1_rx_blocking<R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (MAsyncTx<usize>, R), #[case] tx_count: usize,
) {
    let (tx, rx) = channel;
    let batch_1: usize;
    let batch_2: usize;

    #[cfg(miri)]
    {
        if tx_count > 5 {
            println!("skip");
            return;
        }
        batch_1 = 10;
        batch_2 = 20;
    }
    #[cfg(not(miri))]
    {
        batch_1 = 100;
        batch_2 = 200;
    }
    let rx_res = rx.try_recv();
    assert!(rx_res.is_err());
    assert!(rx_res.unwrap_err().is_empty());
    let th = thread::spawn(move || {
        for _ in 0..((batch_1 + batch_2) * tx_count) {
            match rx.recv() {
                Ok(i) => {
                    debug!("recv {}", i);
                }
                Err(e) => {
                    panic!("error {}", e);
                }
            }
        }
        let res = rx.recv();
        assert!(res.is_err());
        // Wait for spawn exit
    });
    runtime_block_on!(async move {
        let mut th_s = Vec::new();
        for _tx_i in 0..tx_count {
            let _tx = tx.clone();
            th_s.push(async_spawn!(async move {
                for i in 0..batch_1 {
                    let tx_res = _tx.send(i).await;
                    assert!(tx_res.is_ok());
                }
                for i in batch_1..(batch_1 + batch_2) {
                    assert!(_tx.send(10 + i).await.is_ok());
                    sleep(Duration::from_millis(2)).await;
                }
            }));
        }
        drop(tx);

        for th in th_s {
            let _ = async_join_result!(th);
        }
    });
    let _ = th.join().unwrap();
}

#[logfn]
#[rstest]
#[case(spsc::bounded_tx_async_rx_blocking::<usize>(1))]
#[case(spsc::bounded_tx_async_rx_blocking::<usize>(10))]
#[case(spsc::bounded_tx_async_rx_blocking::<usize>(100))]
#[case(spsc::bounded_tx_async_rx_blocking::<usize>(1000))]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(1))]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10))]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(100))]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(1000))]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(1))]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10))]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(100))]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(1000))]
fn test_pressure_1_tx_async_1_rx_blocking<T: AsyncTxTrait<usize>, R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (T, R),
) {
    let (tx, rx) = channel;

    let round: usize;
    #[cfg(miri)]
    {
        round = ROUND;
    }
    #[cfg(not(miri))]
    {
        round = ROUND * 100;
    }

    let th = thread::spawn(move || {
        let mut count = 0;
        'A: loop {
            match rx.recv() {
                Ok(i) => {
                    count += 1;
                    debug!("recv {}", i);
                }
                Err(_) => break 'A,
            }
        }
        debug!("rx exit");
        count
    });
    runtime_block_on!(async move {
        for i in 0..round {
            match tx.send(i).await {
                Err(e) => panic!("{}", e),
                _ => {}
            }
        }
        debug!("tx exit");
    });
    let rx_count = th.join().unwrap();
    assert_eq!(rx_count, round);
}

#[logfn]
#[rstest]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 5)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 10)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(10), 100)]
#[case(mpsc::bounded_tx_async_rx_blocking::<usize>(100), 200)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 5)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 100)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 10)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(100), 200)]
fn test_pressure_multi_tx_async_1_rx_blocking<R: BlockingRxTrait<usize>>(
    setup_log: (), #[case] channel: (MAsyncTx<usize>, R), #[case] tx_count: usize,
) {
    let (tx, rx) = channel;
    #[cfg(miri)]
    {
        if tx_count > 5 {
            println!("skip");
            return;
        }
    }

    let round: usize = ROUND;
    let th = thread::spawn(move || {
        let mut count = 0;
        'A: loop {
            match rx.recv() {
                Ok(_i) => {
                    count += 1;
                    debug!("recv {}", _i);
                }
                Err(_) => break 'A,
            }
        }
        debug!("rx exit");
        count
    });
    runtime_block_on!(async move {
        let mut th_co = Vec::new();
        for _tx_i in 0..tx_count {
            let _tx = tx.clone();
            th_co.push(async_spawn!(async move {
                for i in 0..round {
                    match _tx.send(i).await {
                        Err(e) => panic!("{}", e),
                        _ => {}
                    }
                }
                debug!("tx {} exit", _tx_i);
            }));
        }
        drop(tx);
        for th in th_co {
            let _ = async_join_result!(th);
        }
    });
    let rx_count = th.join().unwrap();
    assert_eq!(rx_count, round * tx_count);
}

#[logfn]
#[rstest]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 5, 5)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 100, 100)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(10), 10, 300)]
#[case(mpmc::bounded_tx_async_rx_blocking::<usize>(100), 300, 300)]
fn test_pressure_multi_tx_async_multi_rx_blocking(
    setup_log: (), #[case] channel: (MAsyncTx<usize>, MRx<usize>), #[case] tx_count: usize,
    #[case] rx_count: usize,
) {
    let (tx, rx) = channel;
    #[cfg(miri)]
    {
        if tx_count > 5 || rx_count > 5 {
            println!("skip");
            return;
        }
    }

    let round: usize = ROUND;
    let mut rx_th_s = Vec::new();
    for _rx_i in 0..rx_count {
        let _rx = rx.clone();
        rx_th_s.push(thread::spawn(move || {
            let mut count = 0;
            'A: loop {
                match _rx.recv() {
                    Ok(i) => {
                        count += 1;
                        debug!("recv {} {}", _rx_i, i);
                    }
                    Err(_) => break 'A,
                }
            }
            debug!("rx {} exit", _rx_i);
            count
        }));
    }
    drop(rx);
    runtime_block_on!(async move {
        let mut th_co = Vec::new();
        for _tx_i in 0..tx_count {
            let _tx = tx.clone();
            th_co.push(async_spawn!(async move {
                for i in 0..round {
                    match _tx.send(i).await {
                        Err(e) => panic!("{}", e),
                        _ => {}
                    }
                }
                debug!("tx {} exit", _tx_i);
            }));
        }
        drop(tx);
        for th in th_co {
            let _ = async_join_result!(th);
        }
    });
    let mut total_count = 0;
    for th in rx_th_s {
        total_count += th.join().unwrap();
    }
    assert_eq!(total_count, round * tx_count);
}
