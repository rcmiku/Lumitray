use std::sync::{Arc, Mutex, mpsc};

use tray_icon_win::{TrayEvent, TrayHandle};

use crate::store::display::DisplayStore;

#[derive(Clone, Default)]
pub(crate) struct TrayStatus {
    pub(crate) name: String,
    pub(crate) brightness: f64,
}

pub(crate) struct TrayStoreInner {
    tray: TrayHandle<TrayStatus>,
    events: Mutex<mpsc::Receiver<TrayEvent>>,
    display: DisplayStore,
}

#[derive(Clone)]
pub(crate) struct TrayStore(Arc<TrayStoreInner>);

impl PartialEq for TrayStore {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl TrayStore {
    pub(crate) fn new(
        tray: TrayHandle<TrayStatus>,
        events: mpsc::Receiver<TrayEvent>,
        display: DisplayStore,
    ) -> Self {
        Self(Arc::new(TrayStoreInner {
            tray,
            events: Mutex::new(events),
            display,
        }))
    }

    pub(crate) fn events(&self) -> &Mutex<mpsc::Receiver<TrayEvent>> {
        &self.0.events
    }

    pub(crate) fn sync(&self) {
        self.0.tray.set_status(TrayStatus {
            name: self.0.display.first_name(),
            brightness: self.0.display.brightness(),
        });
    }
}
