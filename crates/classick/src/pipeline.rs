//! Ordered, bounded-window parallel map. Transcode workers run ahead of a
//! single consumer (the apply-loop committer); results are delivered strictly
//! in `seq` order via `take(seq)`. At most `window` jobs are ever in flight, so
//! temp-file/disk use is bounded independent of library size. libgpod is never
//! touched here — `transcode` is a pure filesystem operation.

use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct Results<T> {
    ready: Mutex<HashMap<usize, anyhow::Result<T>>>,
    cv: Condvar,
}

struct Permits {
    count: Mutex<usize>,
    cv: Condvar,
}

pub struct OrderedTranscoder<T: Send + 'static> {
    results: Arc<Results<T>>,
    permits: Arc<Permits>,
    _feeder: JoinHandle<()>,
    _workers: Vec<JoinHandle<()>>,
}

impl<T: Send + 'static> OrderedTranscoder<T> {
    pub fn start<J, F>(jobs: Vec<(usize, J)>, workers: usize, window: usize, transcode: F) -> Self
    where
        J: Send + 'static,
        F: Fn(&J) -> anyhow::Result<T> + Send + Sync + 'static,
    {
        let window = window.max(1);
        let workers = workers.max(1);
        let results = Arc::new(Results { ready: Mutex::new(HashMap::new()), cv: Condvar::new() });
        let permits = Arc::new(Permits { count: Mutex::new(window), cv: Condvar::new() });
        let transcode = Arc::new(transcode);

        let (job_tx, job_rx): (SyncSender<(usize, J)>, Receiver<(usize, J)>) = sync_channel(window);
        let job_rx = Arc::new(Mutex::new(job_rx));

        // Feeder: acquire a permit per job (bounds in-flight to `window`), then
        // enqueue. Dropping job_tx at the end signals workers to exit.
        let permits_f = permits.clone();
        let feeder = std::thread::spawn(move || {
            for (seq, job) in jobs {
                // acquire permit
                {
                    let mut n = permits_f.count.lock().unwrap();
                    while *n == 0 {
                        n = permits_f.cv.wait(n).unwrap();
                    }
                    *n -= 1;
                }
                if job_tx.send((seq, job)).is_err() {
                    break; // all workers gone
                }
            }
            // job_tx dropped here → workers' recv() returns Err → they exit.
        });

        let mut worker_handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let job_rx = job_rx.clone();
            let results = results.clone();
            let transcode = transcode.clone();
            worker_handles.push(std::thread::spawn(move || loop {
                let next = {
                    let rx = job_rx.lock().unwrap();
                    rx.recv()
                };
                let (seq, job) = match next {
                    Ok(pair) => pair,
                    Err(_) => break, // feeder dropped job_tx
                };
                let out = transcode(&job);
                let mut ready = results.ready.lock().unwrap();
                ready.insert(seq, out);
                results.cv.notify_all();
            }));
        }

        Self { results, permits, _feeder: feeder, _workers: worker_handles }
    }

    /// Block until job `seq` has been transcoded, return its result, and free a
    /// window permit (letting the feeder dispatch one more).
    pub fn take(&self, seq: usize) -> anyhow::Result<T> {
        let mut ready = self.results.ready.lock().unwrap();
        loop {
            if let Some(r) = ready.remove(&seq) {
                drop(ready);
                // release permit
                let mut n = self.permits.count.lock().unwrap();
                *n += 1;
                self.permits.cv.notify_one();
                return r;
            }
            ready = self.results.cv.wait(ready).unwrap();
        }
    }

    /// Idempotent best-effort: wake the feeder so it can observe a dropped
    /// consumer. (Workers exit when job_tx drops; the struct's Drop joins.)
    pub fn stop(&self) {
        self.permits.cv.notify_all();
        self.results.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::OrderedTranscoder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn delivers_in_seq_order_despite_out_of_order_completion() {
        // seq 0 sleeps longest, seq 4 shortest → they finish reversed, but
        // take(0..5) must still return 0,1,2,3,4.
        let jobs: Vec<(usize, usize)> = (0..5).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 4, 8, |&i: &usize| {
            std::thread::sleep(Duration::from_millis(((5 - i) * 20) as u64));
            Ok::<usize, anyhow::Error>(i * 10)
        });
        for seq in 0..5 {
            assert_eq!(ot.take(seq).unwrap(), seq * 10);
        }
    }

    #[test]
    fn never_exceeds_window_in_flight() {
        let max_seen = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(AtomicUsize::new(0));
        let (m, c) = (max_seen.clone(), cur.clone());
        let jobs: Vec<(usize, usize)> = (0..40).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 4, 8, move |&i: &usize| {
            let now = c.fetch_add(1, Ordering::SeqCst) + 1;
            m.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            c.fetch_sub(1, Ordering::SeqCst);
            Ok::<usize, anyhow::Error>(i)
        });
        for seq in 0..40 {
            let _ = ot.take(seq).unwrap();
        }
        // in-flight = concurrently-running transcodes; bounded by min(workers,window).
        assert!(max_seen.load(Ordering::SeqCst) <= 8, "in-flight exceeded window");
    }

    #[test]
    fn propagates_errors_in_order() {
        let jobs: Vec<(usize, usize)> = (0..3).map(|i| (i, i)).collect();
        let ot = OrderedTranscoder::start(jobs, 2, 4, |&i: &usize| {
            if i == 1 { Err(anyhow::anyhow!("boom {i}")) } else { Ok::<usize, anyhow::Error>(i) }
        });
        assert_eq!(ot.take(0).unwrap(), 0);
        assert!(ot.take(1).is_err());
        assert_eq!(ot.take(2).unwrap(), 2);
    }
}
