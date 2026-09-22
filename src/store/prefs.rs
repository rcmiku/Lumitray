use std::sync::Arc;

use crate::prefs::{Prefs, Settings};

pub(crate) struct PrefsStoreInner {
    prefs: Arc<Prefs>,
}

#[derive(Clone)]
pub(crate) struct PrefsStore(Arc<PrefsStoreInner>);

impl PartialEq for PrefsStore {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl PrefsStore {
    pub(crate) fn new(prefs: Arc<Prefs>) -> Self {
        Self(Arc::new(PrefsStoreInner { prefs }))
    }

    pub(crate) fn snapshot(&self) -> Settings {
        self.0.prefs.snapshot()
    }

    pub(crate) fn set(&self, apply: impl FnOnce(&mut Settings)) {
        self.0.prefs.set(apply);
    }

    pub(crate) fn flush_now(&self) {
        self.0.prefs.flush_now();
    }
}
