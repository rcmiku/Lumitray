use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use windows::Win32::Foundation::HWND;

use crate::flyout::FlyoutAnim;

pub(crate) struct WindowStoreInner {
    flyout: Mutex<Option<isize>>,
    flyout_opening: AtomicBool,
    flyout_anim: Mutex<Option<Arc<FlyoutAnim>>>,
    flyout_hidden_at: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone)]
pub(crate) struct WindowStore(pub(crate) Arc<WindowStoreInner>);

impl PartialEq for WindowStore {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl WindowStore {
    pub(crate) fn new() -> Self {
        Self(Arc::new(WindowStoreInner {
            flyout: Mutex::new(None),
            flyout_opening: AtomicBool::new(false),
            flyout_anim: Mutex::new(None),
            flyout_hidden_at: Arc::new(Mutex::new(None)),
        }))
    }

    pub(crate) fn flyout_hwnd(&self) -> Option<HWND> {
        (*self.0.flyout.lock().unwrap()).map(|raw| HWND(raw as *mut _))
    }

    pub(crate) fn publish_flyout(&self, raw: isize) {
        *self.0.flyout.lock().unwrap() = Some(raw);
        self.0.flyout_opening.store(false, Ordering::Release);
    }

    pub(crate) fn claim_flyout_open(&self) -> bool {
        !self.0.flyout_opening.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn flyout_hidden_within(&self, grace: Duration) -> bool {
        self.0.flyout_hidden_within(grace)
    }
}

impl WindowStoreInner {
    pub(crate) fn retire_flyout(&self, raw: isize) {
        {
            let mut flyout = self.flyout.lock().unwrap();
            if *flyout == Some(raw) {
                *flyout = None;
            }
        }
        self.flyout_opening.store(false, Ordering::Release);
    }

    pub(crate) fn set_flyout_anim(&self, anim: Arc<FlyoutAnim>) {
        *self.flyout_anim.lock().unwrap() = Some(anim);
    }

    pub(crate) fn take_flyout_anim(&self) -> Option<Arc<FlyoutAnim>> {
        self.flyout_anim.lock().unwrap().take()
    }

    pub(crate) fn flyout_hidden_at(&self) -> Arc<Mutex<Option<Instant>>> {
        Arc::clone(&self.flyout_hidden_at)
    }

    pub(crate) fn flyout_hidden_within(&self, grace: Duration) -> bool {
        self.flyout_hidden_at
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < grace)
    }
}
