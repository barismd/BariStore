//! Timers: `sleep`, `timeout` and `interval`, as tokio's.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

pub use std::time::Instant;

thread_local! {
    static TIMERS: RefCell<BTreeMap<(Instant, u64), Waker>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Wakes the timers that are due.
pub(crate) fn fire() {
    let now = Instant::now();
    let due: Vec<Waker> = TIMERS.with(|t| {
        let mut t = t.borrow_mut();
        let later = t.split_off(&(now, u64::MAX));
        std::mem::replace(&mut *t, later).into_values().collect()
    });
    for w in due {
        w.wake();
    }
}

/// When the next timer is due.
pub(crate) fn next() -> Option<Instant> {
    TIMERS.with(|t| t.borrow().keys().next().map(|k| k.0))
}

pub struct Sleep {
    deadline: Instant,
    key: Option<(Instant, u64)>,
}

impl Sleep {
    fn unregister(&mut self) {
        if let Some(key) = self.key.take() {
            let _ = TIMERS.try_with(|t| t.borrow_mut().remove(&key));
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if Instant::now() >= self.deadline {
            self.unregister();
            return Poll::Ready(());
        }
        let key = match self.key {
            Some(k) => k,
            None => {
                let id = NEXT_ID.with(|n| {
                    n.set(n.get() + 1);
                    n.get()
                });
                (self.deadline, id)
            }
        };
        self.key = Some(key);
        TIMERS.with(|t| t.borrow_mut().insert(key, cx.waker().clone()));
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        self.unregister();
    }
}

pub fn sleep(d: Duration) -> Sleep {
    sleep_until(Instant::now() + d)
}

pub fn sleep_until(deadline: Instant) -> Sleep {
    Sleep { deadline, key: None }
}

/// The time ran out first.
#[derive(Debug)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

pub async fn timeout<F: Future>(d: Duration, future: F) -> Result<F::Output, Elapsed> {
    let mut future = std::pin::pin!(future);
    let mut timer = std::pin::pin!(sleep(d));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(out) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(out));
        }
        timer.as_mut().poll(cx).map(|()| Err(Elapsed))
    }).await
}

/// Ticks every `period`, the first at once; a late tick is not made up for.
pub struct Interval {
    next: Instant,
    period: Duration,
}

impl Interval {
    pub async fn tick(&mut self) -> Instant {
        sleep_until(self.next).await;
        let now = Instant::now();
        self.next = (self.next + self.period).max(now);
        now
    }
}

pub fn interval(period: Duration) -> Interval {
    Interval { next: Instant::now(), period }
}
