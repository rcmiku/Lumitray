use std::sync::{
    Arc, Mutex,
    mpsc::{Receiver, Sender},
};

use crate::brightness::{Command, MonitorStatus};

pub(crate) struct DisplayStoreInner {
    cmd: Sender<Command>,
    monitor_status: Arc<Mutex<MonitorStatus>>,
    level_notify: Sender<()>,
    level_events: Mutex<Receiver<()>>,
    brightness_events: Mutex<Receiver<f64>>,
    brightness: Mutex<f64>,
}

#[derive(Clone)]
pub(crate) struct DisplayStore(Arc<DisplayStoreInner>);

impl PartialEq for DisplayStore {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl DisplayStore {
    pub(crate) fn new(
        cmd: Sender<Command>,
        monitor_status: Arc<Mutex<MonitorStatus>>,
        level_notify: Sender<()>,
        level_events: Receiver<()>,
        brightness_events: Receiver<f64>,
        brightness: f64,
    ) -> Self {
        Self(Arc::new(DisplayStoreInner {
            cmd,
            monitor_status,
            level_notify,
            level_events: Mutex::new(level_events),
            brightness_events: Mutex::new(brightness_events),
            brightness: Mutex::new(brightness),
        }))
    }

    pub(crate) fn brightness(&self) -> f64 {
        *self.0.brightness.lock().unwrap()
    }

    pub(crate) fn set_brightness(&self, level: f64) {
        *self.0.brightness.lock().unwrap() = level;
    }

    pub(crate) fn adjust_all(&self, step: f64) {
        let _ = self.0.cmd.send(Command::AdjustAll(step));
    }

    pub(crate) fn set(&self, index: usize, level: f64) {
        let _ = self.0.cmd.send(Command::Set(index, level));
    }

    pub(crate) fn set_all(&self, level: f64) {
        let _ = self.0.cmd.send(Command::SetAll(level));
    }

    pub(crate) fn refresh(&self) {
        let _ = self.0.cmd.send(Command::Refresh);
    }

    pub(crate) fn kick_level_sync(&self) {
        let _ = self.0.level_notify.send(());
    }

    pub(crate) fn level_events(&self) -> &Mutex<Receiver<()>> {
        &self.0.level_events
    }

    pub(crate) fn brightness_events(&self) -> &Mutex<Receiver<f64>> {
        &self.0.brightness_events
    }

    pub(crate) fn monitor_count(&self) -> f64 {
        self.0.monitor_status.lock().unwrap().names.len().max(1) as f64
    }

    pub(crate) fn first_name(&self) -> String {
        self.0
            .monitor_status
            .lock()
            .unwrap()
            .names
            .first()
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn levels_and_names(&self) -> (Vec<String>, Vec<f64>) {
        let status = self.0.monitor_status.lock().unwrap();
        (status.names.clone(), status.levels.clone())
    }
}
