use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{mpsc, Condvar};
use std::time::{Duration, Instant};

const PATIENCE: Duration = Duration::from_secs(20);

/// Ticks until `n` jobs have come back, or `PATIENCE` runs out.
fn collect<K, J, R>(pool: &Pool<K, J, R>, n: usize) -> Vec<(K, J, Option<R>)>
where
    K: PartialEq + Send + 'static,
    J: Send + 'static,
    R: Send + 'static,
{
    let end = Instant::now() + PATIENCE;
    let mut all = Vec::new();
    while all.len() < n && Instant::now() < end {
        all.extend(pool.tick());
        std::thread::yield_now();
    }
    all
}

fn threads<K, J, R>(pool: &Pool<K, J, R>) -> usize {
    unpoison(pool.shared.state.lock()).threads
}

/// Whether the `i`th live entry is still waiting for a thread.
fn queued<K, J, R>(pool: &Pool<K, J, R>, i: usize) -> bool {
    unpoison(pool.shared.state.lock()).entries[i].queued.is_some()
}

/// A job that reports it started, then waits for its release; dropping `release` frees every waiting job.
struct Gate {
    started: mpsc::Receiver<u32>,
    release: mpsc::Sender<()>,
}

fn gated(cap: usize) -> (Pool<u32, u32, u32>, Gate, Arc<AtomicUsize>) {
    let (started_tx, started) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let (wait, started_tx) = (Mutex::new(wait), Mutex::new(started_tx));
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (r, p) = (Arc::clone(&running), Arc::clone(&peak));
    let pool = Pool::new(cap, move |job: &u32| {
        p.fetch_max(r.fetch_add(1, SeqCst) + 1, SeqCst);
        started_tx.lock().unwrap().send(*job).unwrap();
        // Released by a send or by the sender dropping; the receiver is shared, so waiters take turns.
        let _ = wait.lock().unwrap().recv();
        r.fetch_sub(1, SeqCst);
        *job
    });
    (pool, Gate { started, release }, peak)
}

#[test]
fn at_most_cap_jobs_run_at_once_and_queued_jobs_start_without_a_tick() {
    let (pool, gate, peak) = gated(2);
    for n in 0..5 {
        assert!(pool.request(n, n));
    }
    let mut first = vec![gate.started.recv_timeout(PATIENCE).unwrap(), gate.started.recv_timeout(PATIENCE).unwrap()];
    first.sort();
    assert_eq!(first, [0, 1]);
    assert_eq!(threads(&pool), 2);
    assert!(queued(&pool, 2), "no third job while two are held");
    // Releasing one job hands its thread the next queued job, with no tick in between.
    gate.release.send(()).unwrap();
    gate.started.recv_timeout(PATIENCE).unwrap();
    drop(gate.release);
    let mut keys: Vec<u32> = collect(&pool, 5).into_iter().map(|(k, ..)| k).collect();
    keys.sort();
    assert_eq!(keys, [0, 1, 2, 3, 4]);
    assert_eq!(peak.load(SeqCst), 2);
}

#[test]
fn a_live_keys_repeat_is_dropped_and_the_key_is_free_after_tick() {
    let pool = Pool::new(2, |job: &u32| *job * 10);
    assert!(pool.request(1, 1));
    assert!(!pool.request(1, 2), "live");
    assert_eq!(pool.pending(), 1);
    let done = collect(&pool, 1);
    assert_eq!(done.len(), 1);
    assert_eq!((done[0].0, done[0].1, done[0].2), (1, 1, Some(10)), "the first job ran, not the repeat");
    assert_eq!(pool.pending(), 0);
    assert!(pool.request(1, 3), "free once tick returned it");
    assert_eq!(collect(&pool, 1)[0].2, Some(30));
}

/// Defect 5: a queued job that is cancelled and requested again must run once, not twice.
#[test]
fn cancel_of_a_queued_job_never_runs_it() {
    let (pool, gate, _) = gated(1);
    assert!(pool.request(1, 1));
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 1);
    assert!(pool.request(2, 2), "queued behind the running job");
    pool.cancel(&2);
    assert_eq!(pool.pending(), 1);
    assert!(pool.request(2, 3), "the key is free at once");
    drop(gate.release);
    let mut done: Vec<(u32, u32)> = collect(&pool, 2).into_iter().map(|(k, j, _)| (k, j)).collect();
    done.sort();
    assert_eq!(done, [(1, 1), (2, 3)]);
    let seen: Vec<u32> = gate.started.try_iter().collect();
    assert_eq!(seen, [3], "job 2 never ran; only its replacement started after job 1");
    assert!(pool.tick().is_empty());
}

#[test]
fn cancel_of_a_running_job_drops_its_result() {
    let (pool, gate, _) = gated(2);
    assert!(pool.request(1, 1));
    gate.started.recv_timeout(PATIENCE).unwrap();
    pool.cancel(&1);
    assert_eq!(pool.pending(), 0);
    assert!(pool.request(1, 2), "a new job for the key runs beside the old one");
    gate.started.recv_timeout(PATIENCE).unwrap();
    drop(gate.release);
    pool.wait_for_threads(0);
    let done = pool.tick();
    assert_eq!(done.len(), 1);
    assert_eq!((done[0].1, done[0].2), (2, Some(2)), "only the new job's result");
}

#[test]
fn cancel_of_a_finished_job_not_yet_taken_drops_its_result() {
    let pool = Pool::new(1, |job: &u32| *job);
    assert!(pool.request(1, 1));
    pool.wait_for_threads(0);
    pool.cancel(&1);
    assert_eq!(pool.pending(), 0);
    assert!(pool.tick().is_empty());
    assert!(pool.request(1, 2));
    assert_eq!(collect(&pool, 1)[0].1, 2);
}

#[test]
fn jobs_start_and_return_in_request_order() {
    let (pool, gate, _) = gated(1);
    for n in 0..4 {
        pool.request(n, n);
    }
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 0);
    pool.cancel(&1); // Two entries follow it, so a `swap_remove` reorders them.
    drop(gate.release);
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 2);
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 3);
    pool.wait_for_threads(0);
    assert_eq!(pool.tick().into_iter().map(|(k, ..)| k).collect::<Vec<_>>(), [0, 2, 3]);
}

#[test]
fn a_burst_past_the_cap_all_comes_back() {
    let pool = Pool::new(2, |job: &u32| *job);
    for n in 0..500 {
        assert!(pool.request(n, n));
    }
    pool.wait_for_threads(0);
    assert_eq!(pool.tick().len(), 500);
}

/// Defect 1: a panic is a result for that job, and the pool keeps working.
#[test]
fn a_panic_delivers_none_with_its_job_and_the_pool_keeps_working() {
    let pool = Pool::new(1, |job: &u32| {
        assert!(*job != 0, "stub job panics (expected in this test)");
        *job
    });
    assert!(pool.request(7, 0));
    assert!(pool.request(8, 1));
    let mut done = collect(&pool, 2);
    done.sort_by_key(|d| d.0);
    assert_eq!((done[0].0, done[0].1, done[0].2), (7, 0, None));
    assert_eq!((done[1].0, done[1].1, done[1].2), (8, 1, Some(1)), "the one thread survived the panic");
    assert_eq!(pool.pending(), 0);
}

#[test]
fn a_cancelled_job_that_panics_keeps_its_thread() {
    let (started_tx, started) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let (started_tx, wait) = (Mutex::new(started_tx), Mutex::new(wait));
    let pool = Pool::new(1, move |job: &u32| {
        if *job == 1 {
            started_tx.lock().unwrap().send(()).unwrap();
            let _ = wait.lock().unwrap().recv();
            panic!("stub job panics (expected in this test)");
        }
        *job
    });
    assert!(pool.request(1, 1));
    started.recv_timeout(PATIENCE).unwrap();
    assert!(pool.request(2, 2));
    pool.cancel(&1);
    drop(release);
    let done = collect(&pool, 1);
    assert_eq!((done[0].0, done[0].1, done[0].2), (2, 2, Some(2)), "the panicking job's thread took the next job");
    pool.wait_for_threads(0);
}

#[test]
fn threads_exit_when_the_queue_empties_and_a_later_request_still_runs() {
    let pool = Pool::new(2, |job: &u32| *job);
    for n in 0..4 {
        pool.request(n, n);
    }
    assert_eq!(collect(&pool, 4).len(), 4);
    pool.wait_for_threads(0);
    assert!(pool.request(9, 9));
    assert_eq!(collect(&pool, 1).len(), 1);
}

#[test]
fn a_dropped_pool_lets_its_threads_finish() {
    let (started_tx, started) = mpsc::channel();
    let (release, wait) = mpsc::channel::<()>();
    let (done_tx, done) = mpsc::channel();
    let (started_tx, wait, done_tx) = (Mutex::new(started_tx), Mutex::new(wait), Mutex::new(done_tx));
    let pool = Pool::new(1, move |job: &u32| {
        started_tx.lock().unwrap().send(()).unwrap();
        let _ = wait.lock().unwrap().recv();
        done_tx.lock().unwrap().send(()).unwrap();
        *job
    });
    assert!(pool.request(1, 1));
    let watch = Pool { shared: Arc::clone(&pool.shared) };
    started.recv_timeout(PATIENCE).unwrap();
    drop(pool);
    drop(release);
    done.recv_timeout(PATIENCE).unwrap();
    watch.wait_for_threads(0);
}

/// Cancelling a running job does not free its thread's cap slot.
#[test]
fn cap_holds_when_a_running_job_is_cancelled() {
    let (pool, gate, peak) = gated(1);
    assert!(pool.request(1, 1));
    assert_eq!(gate.started.recv_timeout(PATIENCE).unwrap(), 1);
    pool.cancel(&1);
    assert_eq!(threads(&pool), 1);
    assert!(pool.request(2, 2));
    assert_eq!(threads(&pool), 1);
    assert!(queued(&pool, 0), "cap 1: job 2 waits for the thread");
    drop(gate.release);
    assert_eq!(collect(&pool, 1)[0].1, 2);
    assert_eq!(peak.load(SeqCst), 1);
}

/// Cap 1, request the instant the previous result is visible: the pool's one thread is then exiting. A request
/// that finds the exiting thread still counted queues a job nobody will run.
#[test]
fn request_racing_a_thread_exit_is_never_stranded() {
    let pool = Pool::new(1, |job: &u32| *job);
    for n in 0..20_000u32 {
        assert!(pool.request(n, n));
        let end = Instant::now() + PATIENCE;
        let mut got = 0;
        while got == 0 && Instant::now() < end {
            got = pool.tick().len();
            std::thread::yield_now();
        }
        assert_eq!(got, 1, "job {n} was stranded");
    }
}

const SEEDS: u64 = 3_000;

/// A seeded random run of `request`, `cancel`, cancel-then-request and `tick`, with gated and panicking jobs, checked
/// after each step against a plain model of which key is live with which request.
#[test]
fn random_runs_match_a_plain_model() {
    for seed in 0..SEEDS {
        run_seed(seed).unwrap_or_else(|why| panic!("seed {seed}: {why}"));
    }
}

static HANG: AtomicUsize = AtomicUsize::new(0);

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..4 {
            r.next();
        }
        r
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// What holds a model job back until the run opens it.
struct Door {
    open: Mutex<bool>,
    cv: Condvar,
}

impl Door {
    fn new(open: bool) -> Arc<Door> {
        Arc::new(Door { open: Mutex::new(open), cv: Condvar::new() })
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn wait(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut open = self.open.lock().unwrap();
        while !*open {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                HANG.fetch_add(1, SeqCst);
                return;
            }
            open = self.cv.wait_timeout(open, left).unwrap().0;
        }
    }
}

struct Job {
    key: u32,
    seq: u64,
    door: Arc<Door>,
    panics: bool,
}

struct Res {
    seq: u64,
}

fn check_tick(pool: &Pool<u32, Job, Res>, live: &mut HashMap<u32, u64>, delivered: &mut u64) -> Result<(), String> {
    for (k, job, res) in pool.tick() {
        if job.key != k {
            return Err(format!("delivered key {k} with job of key {}", job.key));
        }
        match live.get(&k) {
            None => return Err(format!("delivered key {k} seq {} but key not live (cancelled or duplicate)", job.seq)),
            Some(&s) if s != job.seq => {
                return Err(format!("delivered key {k} seq {} but newest live request is seq {s}", job.seq))
            }
            _ => {}
        }
        if job.panics {
            if res.is_some() {
                return Err(format!("panicking job seq {} delivered Some", job.seq));
            }
        } else {
            match &res {
                Some(r) if r.seq == job.seq => {}
                _ => return Err(format!("job seq {} delivered wrong result", job.seq)),
            }
        }
        *delivered += 1;
        live.remove(&k);
    }
    Ok(())
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        if Instant::now() > deadline {
            return Err(format!("timeout waiting for {what}"));
        }
        std::thread::yield_now();
    }
    Ok(())
}

/// One seed in four: every door open, requests and ticks only, so threads exit and restart as fast as they can.
fn is_race(seed: u64) -> bool {
    seed % 4 == 3
}

fn run_seed(seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let cap = 1 + rng.below(3) as usize;
    let nkeys = if is_race(seed) { 6 } else { 2 + rng.below(4) as u32 };
    let nops = 20 + rng.below(80);
    let running = Arc::new(AtomicUsize::new(0));
    let max_running = Arc::new(AtomicUsize::new(0));
    let (r2, m2) = (running.clone(), max_running.clone());
    let pool: Pool<u32, Job, Res> = Pool::new(cap, move |j: &Job| {
        let n = r2.fetch_add(1, SeqCst) + 1;
        m2.fetch_max(n, SeqCst);
        j.door.wait();
        r2.fetch_sub(1, SeqCst);
        assert!(!j.panics, "model job panics (expected in this test)");
        Res { seq: j.seq }
    });
    let mut live: HashMap<u32, u64> = HashMap::new();
    let mut doors: Vec<Arc<Door>> = Vec::new();
    let mut seq = 0u64;
    let mut accepted = 0u64;
    let mut cancelled_live = 0u64;
    let mut delivered = 0u64;

    let mut request = |live: &mut HashMap<u32, u64>,
                       doors: &mut Vec<Arc<Door>>,
                       rng: &mut Rng,
                       key: u32,
                       accepted: &mut u64|
     -> Result<(), String> {
        seq += 1;
        let door = Door::new(rng.below(3) == 0 || is_race(seed));
        doors.push(door.clone());
        let panics = rng.below(6) == 0;
        let expect = !live.contains_key(&key);
        let got = pool.request(key, Job { key, seq, door, panics });
        if got != expect {
            return Err(format!("request(key {key}) returned {got}, model says {expect}"));
        }
        if got {
            live.insert(key, seq);
            *accepted += 1;
        }
        Ok(())
    };

    for _ in 0..nops {
        let key = rng.below(u64::from(nkeys)) as u32;
        let r = rng.below(100);
        // Odd seeds trade most of the cancels for requests and cancel-then-requests.
        let r = match r {
            35..=39 if seed % 2 == 1 => 35,
            40..=44 if seed % 2 == 1 => 50,
            45..=64 if seed % 2 == 1 => 0,
            _ => r,
        };
        let r = match r {
            _ if !is_race(seed) => r,
            0..=59 => 0,
            60..=94 => 65,
            _ => 95,
        };
        match r {
            0..=34 => request(&mut live, &mut doors, &mut rng, key, &mut accepted)?,
            35..=49 => {
                pool.cancel(&key);
                if live.remove(&key).is_some() {
                    cancelled_live += 1;
                }
            }
            50..=64 => {
                pool.cancel(&key);
                if live.remove(&key).is_some() {
                    cancelled_live += 1;
                }
                request(&mut live, &mut doors, &mut rng, key, &mut accepted)?;
                if !live.contains_key(&key) {
                    return Err("cancel+request did not take".into());
                }
            }
            65..=79 => check_tick(&pool, &mut live, &mut delivered)?,
            80..=94 => {
                if !doors.is_empty() {
                    let i = rng.below(doors.len() as u64) as usize;
                    doors[i].open();
                }
            }
            _ => std::thread::yield_now(),
        }
        let p = pool.pending();
        if p != live.len() {
            return Err(format!("pending {p} != model live {}", live.len()));
        }
        let t = threads(&pool);
        if t > cap {
            return Err(format!("threads {t} > cap {cap}"));
        }
    }
    for door in &doors {
        door.open();
    }
    let mut drain = |live: &mut HashMap<u32, u64>, what: &str| {
        let mut failed = Ok(());
        let waited = wait_until(what, || {
            failed = check_tick(&pool, live, &mut delivered);
            failed.is_err() || live.is_empty()
        });
        failed?;
        waited.map_err(|why| format!("{why}; still live: {live:?}"))
    };
    drain(&mut live, "every live request delivered")?;
    wait_until("threads to return to 0", || threads(&pool) == 0)?;
    if pool.pending() != 0 {
        return Err("pending != 0 after drain".into());
    }
    // A later request still runs: the wake-up after the exit race.
    request(&mut live, &mut doors, &mut rng, 99, &mut accepted)?;
    doors.last().unwrap().open();
    drain(&mut live, "late request delivered")?;
    wait_until("threads to 0 (late)", || threads(&pool) == 0)?;
    if accepted != cancelled_live + delivered {
        return Err(format!("accounting: accepted {accepted} != cancelled {cancelled_live} + delivered {delivered}"));
    }
    let most = max_running.load(SeqCst);
    if most > cap {
        return Err(format!("concurrent work {most} > cap {cap}"));
    }
    if running.load(SeqCst) != 0 {
        return Err("running != 0 at end".into());
    }
    if HANG.load(SeqCst) != 0 {
        return Err("a door wait timed out".into());
    }
    Ok(())
}
