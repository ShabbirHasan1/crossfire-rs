use crate::collections::ArcCell;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::transmute;
use std::ops::Deref;
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc, Weak,
};
use std::task::*;
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum WakerState {
    Init = 0, // A temporary state, https://github.com/frostyplanet/crossfire-rs/issues/22
    Waiting = 1,
    //Copy = 2, // Omit due to skipping direct copy on async or with deadline
    Waked = 3,
    Closed = 4, // Channel closed, or timeout cancellation
    Done = 5,
}

#[derive(PartialEq, Debug)]
pub enum WakeResult {
    Sent,
    Next,
    Waked,
}

/// Although removing direct copy feature of the payload pointer is not used,
/// leave it to unbuffer channel in the future
pub struct ChannelWaker<P>(Arc<WakerInner<P>>);

impl<P> fmt::Debug for ChannelWaker<P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<P> fmt::Debug for WakerInner<P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "waker({})", self.get_seq())
    }
}

impl<P> Deref for ChannelWaker<P> {
    type Target = WakerInner<P>;
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<P> ChannelWaker<P> {
    #[inline(always)]
    pub fn new_async(ctx: &Context, payload: P) -> Self {
        Self(Arc::new(WakerInner {
            seq: AtomicUsize::new(0),
            state: AtomicU8::new(WakerState::Init as u8),
            waker: UnsafeCell::new(WakerType::Async(ctx.waker().clone())),
            payload: UnsafeCell::new(payload),
        }))
    }

    #[inline(always)]
    pub fn new_blocking(payload: P) -> Self {
        Self(Arc::new(WakerInner {
            seq: AtomicUsize::new(0),
            state: AtomicU8::new(WakerState::Init as u8),
            waker: UnsafeCell::new(WakerType::Blocking(thread::current())),
            payload: UnsafeCell::new(payload),
        }))
    }
}

impl<P> ChannelWaker<P> {
    #[inline(always)]
    pub fn from_arc(inner: Arc<WakerInner<P>>) -> Self {
        Self(inner)
    }

    #[inline(always)]
    pub fn to_arc(self) -> Arc<WakerInner<P>> {
        self.0
    }

    #[inline(always)]
    pub fn weak(&self) -> Weak<WakerInner<P>> {
        Arc::downgrade(&self.0)
    }
}

pub type RecvWaker = ChannelWaker<()>;

pub type SendWaker<T> = ChannelWaker<*const T>;

enum WakerType {
    Async(Waker),
    Blocking(thread::Thread),
}

pub struct WakerInner<P> {
    state: AtomicU8,
    seq: AtomicUsize,
    waker: UnsafeCell<WakerType>,
    #[allow(dead_code)]
    payload: UnsafeCell<P>,
}

unsafe impl<P> Send for WakerInner<P> {}
unsafe impl<P> Sync for WakerInner<P> {}

impl<P> WakerInner<P> {
    #[inline(always)]
    fn get_waker(&self) -> &WakerType {
        unsafe { transmute(self.waker.get()) }
    }

    #[inline(always)]
    fn get_waker_mut(&self) -> &mut WakerType {
        unsafe { transmute(self.waker.get()) }
    }

    #[inline(always)]
    fn get_payload_mut(&self) -> &mut P {
        unsafe { transmute(self.payload.get()) }
    }

    #[inline(always)]
    pub fn reset(&self, payload: P) {
        // From the object pool to reset value,
        // we should use SeqCst fence to clear the cache of other cores
        *self.get_payload_mut() = payload;
        self.reset_init();
    }

    #[inline(always)]
    pub fn get_seq(&self) -> usize {
        self.seq.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn set_seq(&self, seq: usize) {
        self.seq.store(seq, Ordering::Relaxed);
    }

    #[inline(always)]
    fn update_thread_handle(&self) {
        let _waker = self.get_waker_mut();
        *_waker = WakerType::Blocking(thread::current());
    }

    #[inline(always)]
    pub fn commit_waiting(&self) -> u8 {
        if let Err(s) = self.try_change_state(WakerState::Init, WakerState::Waiting) {
            return s;
        } else {
            return WakerState::Waiting as u8;
        }
    }

    #[inline(always)]
    pub fn try_change_state(&self, cur: WakerState, new_state: WakerState) -> Result<(), u8> {
        if let Err(s) = self.state.compare_exchange(
            cur as u8,
            new_state as u8,
            Ordering::SeqCst,
            Ordering::Acquire,
        ) {
            return Err(s);
        }
        return Ok(());
    }

    /// Only used in cancel_wake, it's ok to use Relaxed
    #[inline(always)]
    pub fn set_state_relaxed(&self, state: WakerState) {
        let _state = state as u8;
        #[cfg(all(test, not(feature = "trace_log")))]
        {
            if _state != WakerState::Closed as u8 {
                let __state = self.get_state_relaxed();
                assert!(
                    __state == WakerState::Waked as u8 || __state <= _state as u8,
                    "unexpected set state {:?} on state: {}",
                    _state,
                    __state
                );
            }
        }
        self.state.store(_state, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn reset_init(&self) {
        // this is before we put into registry (which will extablish happen-before relationship),
        // it safe to use Relaxed
        self.state.store(WakerState::Init as u8, Ordering::Relaxed);
    }

    /// Return current status,
    /// Closed: might be channel closed, or future successfully cancelled, the future should drop message; try to clear its waker.
    /// Done: the message actually sent, nothing to DO
    /// Waked: the future should drop message, and waked another counterpart.
    #[inline(always)]
    pub fn abandon(&self) -> Result<(), u8> {
        // it will content with close(), on_recv(), on_send()
        match self.change_state_smaller_eq(WakerState::Waiting, WakerState::Closed) {
            Ok(_) => Ok(()),
            Err(state) => Err(state),
        }
        // NOTE: there's no Copy state, so we do not loop
    }

    #[inline(always)]
    pub fn close_wake(&self) -> bool {
        // should have lock because it will content with abandon()
        if self.change_state_smaller_eq(WakerState::Waiting, WakerState::Closed).is_ok() {
            self._wake_nolock();
            return true;
        }
        return false;
    }

    // Return Ok(pre_state), otherwise return Err(current_state)
    #[inline(always)]
    pub fn change_state_smaller_eq(
        &self, condition: WakerState, target: WakerState,
    ) -> Result<u8, u8> {
        debug_assert!((condition as u8) < (target as u8));
        // Save one load()
        let mut state = condition as u8;
        loop {
            match self.state.compare_exchange_weak(
                state,
                target as u8,
                Ordering::SeqCst,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(state);
                }
                Err(s) => {
                    if s > condition as u8 {
                        return Err(s);
                    }
                    state = s;
                }
            }
        }
    }

    #[inline(always)]
    pub fn get_state(&self) -> u8 {
        self.state.load(Ordering::SeqCst)
    }

    #[inline(always)]
    pub fn get_state_relaxed(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    /// Assume no lock
    #[inline(always)]
    pub fn wake(&self) -> WakeResult {
        // This is after we get waker from waker_registry, which already happen before relationship.
        // both >= WakerState::Waiting is certain
        let mut state = self.get_state_relaxed();
        loop {
            if state >= WakerState::Waked as u8 {
                return WakeResult::Next;
            } else if state == WakerState::Waiting as u8 {
                self.state.store(WakerState::Waked as u8, Ordering::SeqCst);
                self._wake_nolock();
                return WakeResult::Waked;
            } else {
                match self.state.compare_exchange_weak(
                    WakerState::Init as u8,
                    WakerState::Waked as u8,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self._wake_nolock();
                        return WakeResult::Next;
                    }
                    Err(s) => {
                        state = s;
                    }
                }
            }
        }
    }

    pub fn will_wake(&self, ctx: &mut Context) -> bool {
        // ref: https://github.com/frostyplanet/crossfire-rs/issues/14
        // https://docs.rs/tokio/latest/tokio/runtime/index.html#:~:text=Normally%2C%20tasks%20are%20scheduled%20only,is%20called%20a%20spurious%20wakeup
        // There might be situation like spurious wakeup, poll() again under no waking up ever
        // happened, waker still exists in registry but cannot be used to wake the current future.
        let o_waker = self.get_waker();
        if let WakerType::Async(_waker) = o_waker {
            return _waker.will_wake(ctx.waker());
        } else {
            unreachable!();
        }
    }

    // Assume the state change have proper fence
    #[inline(always)]
    fn _wake_nolock(&self) {
        match self.get_waker() {
            WakerType::Async(w) => w.wake_by_ref(),
            WakerType::Blocking(th) => th.unpark(),
        }
    }
}

impl<T> WakerInner<*const T> {
    #[inline(always)]
    fn get_payload(&self) -> *const T {
        *self.get_payload_mut()
    }

    #[inline(always)]
    pub fn wake_or_copy<F>(&self, copy_f: F) -> WakeResult
    where
        F: FnOnce(*const T) -> u8,
    {
        // This is after we get waker from waker_registry, which already happen before relationship.
        // both >= WakerState::Waiting is certain
        let mut state = self.get_state_relaxed();
        loop {
            if state >= WakerState::Waked as u8 {
                return WakeResult::Next;
            } else if state == WakerState::Waiting as u8 {
                let p = self.get_payload();
                if p == std::ptr::null_mut() {
                    self.state.store(WakerState::Waked as u8, Ordering::SeqCst);
                    self._wake_nolock();
                    return WakeResult::Waked;
                }
                state = copy_f(p as *const T);
                self.state.store(state as u8, Ordering::SeqCst);
                self._wake_nolock();
                if state == WakerState::Done as u8 {
                    return WakeResult::Sent;
                } else {
                    return WakeResult::Waked;
                }
            } else {
                match self.state.compare_exchange_weak(
                    WakerState::Init as u8,
                    WakerState::Waked as u8,
                    Ordering::SeqCst,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self._wake_nolock();
                        return WakeResult::Next;
                    }
                    Err(s) => {
                        state = s;
                    }
                }
            }
        }
    }
}

pub struct WakerCache<P: Copy>(ArcCell<WakerInner<P>>);

impl<P: Copy> WakerCache<P> {
    #[inline(always)]
    pub(crate) fn new() -> Self {
        Self(ArcCell::new())
    }

    #[inline(always)]
    pub fn new_blocking(&self, payload: P) -> ChannelWaker<P> {
        if let Some(inner) = self.0.pop() {
            inner.update_thread_handle();
            inner.reset(payload);
            return ChannelWaker::<P>::from_arc(inner);
        }
        return ChannelWaker::new_blocking(payload);
    }

    #[inline(always)]
    pub(crate) fn push(&self, waker: ChannelWaker<P>) {
        debug_assert!(waker.get_state() >= WakerState::Waked as u8);
        let a = waker.to_arc();
        if Arc::weak_count(&a) == 0 && Arc::strong_count(&a) == 1 {
            self.0.try_put(a);
        }
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        !self.0.exists()
    }
}
