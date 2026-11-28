//! BariStore's runtime: async tasks on the one thread rootshell gives a
//! WebAssembly program, in place of tokio's (which needs threads, or a
//! poll the program does not have).
//!
//! * Tasks (`spawn`, `JoinHandle`, `block_on`) run in turn; timers
//!   (`time`) wake them; sockets (`io`) wake them when rootshell brings
//!   bytes or a connection.
//! * While every task waits, the runtime waits in rootshell: on the socket a
//!   task reads (with a receive time limit, so a timer is never late), or on
//!   a socket nothing is sent to. Never in WASI's own sleep, which asks the
//!   app nothing: WebKit could then stop the web view the program runs in
//!   (`crate::rootshell::nudge`).
//! * Long computations (signing, hashing, packing) call `check_in`, which
//!   lets the other tasks run meanwhile (the page keeps answering) and keeps
//!   WebKit going.

pub mod io;
pub mod time;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

type Task = Pin<Box<dyn Future<Output = ()>>>;

/// Woken tasks, by (slot, generation).
type Ready = Arc<Mutex<VecDeque<(usize, u64)>>>;

struct Flag {
    id: usize,
    generation: u64,
    queued: AtomicBool,
    ready: Ready,
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.queued.swap(true, Ordering::SeqCst) {
            lock(&self.ready).push_back((self.id, self.generation));
        }
    }
}

struct Slot {
    /// None while the task is being polled.
    future: Option<Task>,
    generation: u64,
    running: bool,
    /// Woken while it was being polled: poll it again.
    rewake: bool,
    aborted: bool,
    flag: Arc<Flag>,
}

#[derive(Default)]
struct Executor {
    slots: RefCell<Vec<Option<Slot>>>,
    free: RefCell<Vec<usize>>,
    ready: Ready,
    generation: Cell<u64>,
    last_io: Cell<Option<Instant>>,
}

thread_local! {
    static EXECUTOR: Executor = Executor::default();
    static LAST_CHECK_IN: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Executor {
    fn insert(&self, future: Task) -> (usize, u64) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let mut slots = self.slots.borrow_mut();
        let id = self.free.borrow_mut().pop().unwrap_or_else(|| {
            slots.push(None);
            slots.len() - 1
        });
        let flag = Arc::new(Flag { id, generation, queued: AtomicBool::new(true), ready: self.ready.clone() });
        slots[id] = Some(Slot { future: Some(future), generation, running: false, rewake: false, aborted: false, flag });
        lock(&self.ready).push_back((id, generation));
        (id, generation)
    }

    fn pop(&self) -> Option<(usize, u64)> {
        lock(&self.ready).pop_front()
    }

    fn has_ready(&self) -> bool {
        !lock(&self.ready).is_empty()
    }

    fn poll(&self, id: usize, generation: u64) {
        let (mut future, flag) = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(id).and_then(Option::as_mut).filter(|s| s.generation == generation) else {
                return;
            };
            if slot.running {
                slot.rewake = true;
                return;
            }
            let Some(future) = slot.future.take() else { return };
            slot.running = true;
            slot.flag.queued.store(false, Ordering::SeqCst);
            (future, slot.flag.clone())
        };
        let waker = Waker::from(flag);
        let done = future.as_mut().poll(&mut Context::from_waker(&waker)).is_ready();
        let finished = {
            let mut slots = self.slots.borrow_mut();
            let slot = slots[id].as_mut().expect("a running task's slot");
            slot.running = false;
            if done || slot.aborted {
                slots[id] = None;
                self.free.borrow_mut().push(id);
                Some(future)
            } else {
                slot.future = Some(future);
                if std::mem::take(&mut slot.rewake) {
                    slot.flag.queued.store(true, Ordering::SeqCst);
                    lock(&self.ready).push_back((id, generation));
                }
                None
            }
        };
        // Outside the borrow: dropping a future runs its destructors.
        drop(finished);
    }

    fn abort(&self, id: usize, generation: u64) {
        let future = {
            let mut slots = self.slots.borrow_mut();
            match slots.get_mut(id).and_then(Option::as_mut) {
                Some(slot) if slot.generation == generation => {
                    if slot.running {
                        slot.aborted = true;
                        None
                    } else {
                        let future = slot.future.take();
                        slots[id] = None;
                        self.free.borrow_mut().push(id);
                        future
                    }
                }
                _ => None,
            }
        };
        drop(future);
    }

    /// Drops every task left (once the main future ended, as a runtime
    /// that shuts down does), while the timers they may hold still exist.
    fn clear(&self) {
        loop {
            let left: Vec<Task> = self.slots.borrow_mut().iter_mut().filter_map(|s| s.take()?.future).collect();
            if left.is_empty() {
                break;
            }
            drop(left);
        }
        self.free.borrow_mut().clear();
        lock(&self.ready).clear();
    }

    /// Runs up to `budget` woken tasks.
    fn run(&self, budget: usize) {
        for _ in 0..budget {
            let Some((id, generation)) = self.pop() else { return };
            self.poll(id, generation);
        }
    }

    /// One round: woken tasks, due timers, then a wait in rootshell until
    /// something happens or the next timer is due (only a short look while
    /// tasks are still woken, every 10 ms at most).
    fn round(&self, main_woken: &AtomicBool) {
        self.run(128);
        time::fire();
        let now = Instant::now();
        if self.has_ready() || main_woken.load(Ordering::SeqCst) {
            if self.last_io.get().is_none_or(|t| now.duration_since(t) >= Duration::from_millis(10)) {
                io::turn(Some(now));
                self.last_io.set(Some(Instant::now()));
            }
            return;
        }
        io::turn(time::next());
        self.last_io.set(Some(Instant::now()));
        time::fire();
    }
}

struct MainFlag(AtomicBool);

impl Wake for MainFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Runs `future` and every task it spawns until `future` ends.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let main = Arc::new(MainFlag(AtomicBool::new(true)));
    let waker = Waker::from(main.clone());
    loop {
        if main.0.swap(false, Ordering::SeqCst) {
            if let Poll::Ready(out) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                EXECUTOR.with(Executor::clear);
                return out;
            }
        }
        EXECUTOR.with(|ex| ex.round(&main.0));
    }
}

/// The task was aborted before it ended.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the task was stopped")
    }
}

struct Joined<T> {
    result: Option<T>,
    finished: bool,
    waker: Option<Waker>,
}

/// A spawned task: awaited for its result, or aborted. Dropping it lets the
/// task go on.
pub struct JoinHandle<T> {
    joined: Arc<Mutex<Joined<T>>>,
    id: usize,
    generation: u64,
}

impl<T> JoinHandle<T> {
    pub fn abort(&self) {
        EXECUTOR.with(|ex| ex.abort(self.id, self.generation));
        let waker = {
            let mut j = lock(&self.joined);
            j.finished = true;
            j.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, Cancelled>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut j = lock(&self.joined);
        if j.finished {
            return Poll::Ready(j.result.take().ok_or(Cancelled));
        }
        j.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Starts `future` as a task of its own.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where F: Future + 'static, F::Output: 'static
{
    let joined = Arc::new(Mutex::new(Joined { result: None, finished: false, waker: None }));
    let mine = joined.clone();
    let task = async move {
        let out = future.await;
        let waker = {
            let mut j = lock(&mine);
            if !j.finished {
                j.result = Some(out);
                j.finished = true;
            }
            j.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    };
    let (id, generation) = EXECUTOR.with(|ex| ex.insert(Box::pin(task)));
    JoinHandle { joined, id, generation }
}

/// Lets the other woken tasks run first.
#[cfg(test)]
pub async fn yield_now() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if std::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }).await
}

/// Runs a computation in place (there is one thread): it calls `check_in`
/// as it goes, so the page keeps answering meanwhile.
pub fn block_in_place<R>(work: impl FnOnce() -> R) -> R {
    work()
}

/// From inside a long computation, every 50 ms at most: due timers, a
/// short look at the sockets, and up to 32 woken tasks (never the one
/// computing: it is the one being polled). This keeps WebKit going too.
pub fn check_in() {
    let now = Instant::now();
    if LAST_CHECK_IN.with(|l| l.get()).is_some_and(|t| now.duration_since(t) < Duration::from_millis(50)) {
        return;
    }
    LAST_CHECK_IN.with(|l| l.set(Some(now)));
    EXECUTOR.with(|ex| {
        time::fire();
        io::turn(Some(now));
        ex.run(32);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn tasks_timers_and_results() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let o = order.clone();
        let out = block_on(async move {
            let a = { let o = o.clone(); spawn(async move { time::sleep(Duration::from_millis(30)).await; o.borrow_mut().push("a"); 1 }) };
            let b = { let o = o.clone(); spawn(async move { time::sleep(Duration::from_millis(5)).await; o.borrow_mut().push("b"); 2 }) };
            let never = spawn(async { time::sleep(Duration::from_secs(60)).await; 3 });
            never.abort();
            (a.await.unwrap() + b.await.unwrap(), never.await.is_err())
        });
        assert_eq!(out, (3, true));
        assert_eq!(*order.borrow(), ["b", "a"]);
    }

    #[test]
    fn timeouts_end_waits() {
        block_on(async {
            let started = Instant::now();
            let r = time::timeout(Duration::from_millis(20), time::sleep(Duration::from_secs(5))).await;
            assert!(r.is_err() && started.elapsed() < Duration::from_secs(2));
            assert_eq!(time::timeout(Duration::from_secs(5), async { 7 }).await.unwrap(), 7);
        });
    }

    #[test]
    fn check_in_runs_other_tasks_during_a_computation() {
        let ticks = Rc::new(Cell::new(0));
        let t = ticks.clone();
        block_on(async move {
            let counter = spawn(async move {
                loop {
                    time::sleep(Duration::from_millis(10)).await;
                    t.set(t.get() + 1);
                }
            });
            yield_now().await;
            let started = Instant::now();
            block_in_place(|| while started.elapsed() < Duration::from_millis(400) { check_in(); });
            counter.abort();
        });
        assert!(ticks.get() >= 3, "{} ticks", ticks.get());
    }
}
