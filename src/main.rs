#![cfg_attr(
    all(not(test), not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod auto_start;
mod brightness;
mod flyout;
mod prefs;
mod store;
mod tray;
mod tray_host;
mod win32;

use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
};

use windows::core::w;
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError},
    System::Threading::CreateMutexW,
    UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
};
use windows_reactor::{App, View};

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let single_instance = unsafe {
        let handle = CreateMutexW(
            None,
            true,
            w!("Local\\Lumitray_SingleInstance_Mutex"),
        );
        if handle.is_err() || GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        handle.unwrap()
    };

    let (tray, events) = tray::start_tray().expect("failed to start the tray icon");
    let monitor_status = Arc::new(Mutex::new(brightness::MonitorStatus::default()));
    let (flyout_level_notify, flyout_level_events) = mpsc::channel();
    let (brightness_notify, brightness_events) = mpsc::channel::<f64>();
    let brightness = brightness::start_worker(
        Arc::clone(&monitor_status),
        flyout_level_notify.clone(),
        brightness_notify,
    );
    let prefs = Arc::new(prefs::Prefs::load());
    let display = store::display::DisplayStore::new(
        brightness,
        monitor_status,
        flyout_level_notify,
        flyout_level_events,
        brightness_events,
        prefs.snapshot().brightness,
    );
    auto_start::refresh();
    if prefs.snapshot().restore_brightness {
        display.set_all(prefs.snapshot().brightness);
    } else {
        display.refresh();
    }
    let prefs = store::prefs::PrefsStore::new(prefs);
    let window = store::window::WindowStore::new();
    let tray = store::tray::TrayStore::new(tray, events, display.clone());
    let keep_alive = tray.clone();
    App::run_with(move |app| {
        app.open_window(View::component::<tray_host::TrayHost>(
            tray_host::TrayHostInput {
                app: Rc::new(app.clone()),
                display: display.clone(),
                prefs: prefs.clone(),
                tray: tray.clone(),
                window: window.clone(),
            },
        ))?;
        Ok(keep_alive)
    })
    .unwrap();

    unsafe {
        let _ = CloseHandle(single_instance);
    }
}
