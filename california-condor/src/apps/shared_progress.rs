use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
    Mutex,
};

/// Thread-safe container for sharing mutable progress state between a
/// producer thread (progress receiver) and a consumer thread (UI event loop).
///
/// The producer calls [`SharedProgress::apply`] to update state. The consumer
/// calls [`SharedProgress::read_if_dirty`] once per tick to retrieve the
/// latest snapshot.
///
/// # Type parameters
/// * `State` — The concrete progress state struct. Must be cloneable and `Send
///   + 'static` so it can be shared across threads.
pub struct SharedProgress<State: Clone + Send + 'static> {
    state: Arc<Mutex<State>>,
    dirty: Arc<AtomicBool>,
}

impl<State: Clone + Send + 'static> SharedProgress<State> {
    pub fn new(initial: State) -> Self {
        Self {
            state: Arc::new(Mutex::new(initial)),
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Apply an update function to the shared state.
    ///
    /// The `update` closure receives a mutable reference to the inner state and
    /// returns `true` if the state was meaningfully changed (setting the dirty
    /// flag so the consumer will pick it up on the next tick).
    pub fn apply(&self, update: impl FnOnce(&mut State) -> bool) {
        let mut guard = self.state.lock().expect("SharedProgress lock");
        if update(&mut *guard) {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// Read-and-clear the dirty flag.
    ///
    /// Returns `Some(state)` if a new snapshot is available since the last
    /// call, or `None` if nothing has changed.
    pub fn read_if_dirty(&self) -> Option<State> {
        if self.dirty.swap(false, Ordering::Acquire) {
            let guard = self.state.lock().expect("SharedProgress lock");
            return Some(guard.clone());
        }
        None
    }

    /// Force-read the current state, ignoring the dirty flag.
    pub fn read(&self) -> State {
        let guard = self.state.lock().expect("SharedProgress lock");
        guard.clone()
    }
}

// Every clone shares the same `Arc`-backed state.
impl<S: Clone + Send + 'static> Clone for SharedProgress<S> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            dirty: Arc::clone(&self.dirty),
        }
    }
}
