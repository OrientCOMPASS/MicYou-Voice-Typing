// src/state_machine.rs
use std::sync::{Arc, atomic::{AtomicI64, AtomicBool, Ordering}};

#[derive(Debug, Clone)]
pub struct TimeWindowStateMachine {
    pub is_active: Arc<AtomicBool>,
    pub current_session_start: Arc<AtomicI64>,
    pub last_session_start: Arc<AtomicI64>,
    pub last_session_end: Arc<AtomicI64>,
    pub grace_period_ms: i64,
}

impl TimeWindowStateMachine {
    pub fn new(grace_period_ms: i64) -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
            current_session_start: Arc::new(AtomicI64::new(0)),
            last_session_start: Arc::new(AtomicI64::new(0)),
            last_session_end: Arc::new(AtomicI64::new(0)),
            grace_period_ms,
        }
    }

    pub fn on_activate(&self, now: i64) {
        if !self.is_active.swap(true, Ordering::SeqCst) {
            self.current_session_start.store(now, Ordering::SeqCst);
        }
    }

    pub fn on_deactivate(&self, now: i64) {
        if self.is_active.swap(false, Ordering::SeqCst) {
            self.last_session_start.store(self.current_session_start.load(Ordering::SeqCst), Ordering::SeqCst);
            self.last_session_end.store(now, Ordering::SeqCst);
        }
    }

    pub fn should_inject(&self, w_start: i64, w_end: i64) -> bool {
        if self.is_active.load(Ordering::SeqCst) {
            return w_end >= self.current_session_start.load(Ordering::SeqCst);
        }

        let effective_window_end = self.last_session_end.load(Ordering::SeqCst) + self.grace_period_ms;
        let last_start = self.last_session_start.load(Ordering::SeqCst);
        
        w_start <= effective_window_end && w_end >= last_start
    }
}