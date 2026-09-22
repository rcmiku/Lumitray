use std::{
    ffi::c_void,
    mem::{size_of, size_of_val},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmFlush, DwmSetWindowAttribute,
    },
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST, HWND_TOPMOST, SW_HIDE,
            SW_SHOW, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WA_INACTIVE,
            WHEEL_DELTA, WM_ACTIVATE, WM_NCDESTROY, WS_EX_TOOLWINDOW,
        },
    },
};
use windows_animation::{Manager, TransitionLibrary};
use windows_reactor::*;

use crate::tray_host::BRIGHTNESS_WHEEL_STEP;

use crate::{
    store::{
        display::DisplayStore,
        window::{WindowStore, WindowStoreInner},
    },
    win32::GWLP_EXSTYLE,
};

const FLYOUT_TITLE: &str = "Lumitray Flyout";
const FLYOUT_WIDTH: f64 = 340.0;
const FLYOUT_PADDING: f64 = 12.0;
const FLYOUT_BASE_HEIGHT: f64 = 2.0 * FLYOUT_PADDING;
const FLYOUT_ROW_HEIGHT: f64 = 56.0;
const FLYOUT_MAX_MONITORS: usize = 4;
const FLYOUT_SUBCLASS_ID: usize = 2;
pub(crate) const REOPEN_GRACE: Duration = Duration::from_millis(400);
const FLYOUT_ANIM_DURATION: Duration = Duration::from_millis(180);

pub(crate) struct Brightness {
    names: Vec<String>,
    levels: Vec<f64>,
    display: DisplayStore,
    window: WindowStore,
}

#[derive(Clone, PartialEq)]
pub(crate) struct BrightnessInput {
    pub(crate) display: DisplayStore,
    pub(crate) window: WindowStore,
}

pub(crate) enum Message {
    WindowReady(isize),
    Set { index: usize, level: f64 },
    Wheel { index: usize, delta: i32 },
    Synced(Option<(Vec<String>, Vec<f64>)>),
}

impl Component for Brightness {
    type Input = BrightnessInput;
    type Message = Message;

    fn create(input: &BrightnessInput, context: &ComponentContext<Self>) -> Self {
        if !context.run_window(|window| Message::WindowReady(window.as_raw() as isize)) {
            eprintln!("flyout: 窗口句柄获取未排上，飞窗外观/定位不会安装");
        }
        wait_for_level_change(&input.display, context);
        let (names, levels) = input.display.levels_and_names();
        Self {
            names,
            levels,
            display: input.display.clone(),
            window: input.window.clone(),
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::WindowReady(raw) => {
                install_flyout_window(&self.display, &self.window, HWND(raw as *mut _));
            }
            Message::Wheel { index, delta } => {
                let steps = f64::from(delta) / f64::from(WHEEL_DELTA) * BRIGHTNESS_WHEEL_STEP;
                if let Some(existing) = self.levels.get_mut(index) {
                    let level = (*existing + steps).clamp(0.0, 100.0);
                    *existing = level;
                    self.display.set(index, level);
                }
            }
            Message::Set { index, level } => {
                if let Some(existing) = self.levels.get_mut(index) {
                    *existing = level;
                    self.display.set(index, level);
                }
            }
            Message::Synced(Some((names, levels))) => {
                self.names = names;
                self.levels = levels;
                wait_for_level_change(&self.display, context);
            }
            Message::Synced(None) => {}
        }
    }

    fn view(&self, _input: &BrightnessInput, context: &mut ViewContext<Self>) -> View {
        let height = FLYOUT_BASE_HEIGHT + FLYOUT_ROW_HEIGHT * self.names.len().max(1) as f64;
        context.window_visuals(
            WindowVisuals::new()
                .backdrop(WindowBackdrop::Acrylic)
                .border(true)
                .title_bar(false)
                .resizable(false)
                .shown_in_switchers(false)
                .client_size(FLYOUT_WIDTH, height),
        );
        context.window_title(FLYOUT_TITLE);
        let rows: [View; FLYOUT_MAX_MONITORS] = std::array::from_fn(|index| {
            let Some(name) = self.names.get(index) else {
                return View::empty();
            };
            let level = self.levels.get(index).copied().unwrap_or(50.0);
            let name = if name.is_empty() {
                format!("显示器 {}", index + 1)
            } else {
                name.clone()
            };
            StackPanel::new().spacing(8.0).children((
                StackPanel::new()
                    .orientation(Orientation::Horizontal)
                    .spacing(8.0)
                    .children((monitor_icon(), TextBlock::new().text(name))),
                Slider::new()
                    .minimum(0.0)
                    .maximum(100.0)
                    .value(level)
                    .width(FLYOUT_WIDTH - 2.0 * FLYOUT_PADDING - 3.0)
                    .horizontal_alignment(HorizontalAlignment::Center)
                    .on_value_changed(context.callback(move |value: f64| Message::Set {
                        index,
                        level: value,
                    }))
                    .on_pointer_wheel(context.callback(move |info: PointerEventInfo| {
                        Message::Wheel {
                            index,
                            delta: info.wheel_delta,
                        }
                    })),
            ))
        });
        let content: View = if self.names.is_empty() {
            TextBlock::new()
                .text("未检测到支持 DDC/CI 的显示器")
                .opacity(0.7)
                .into()
        } else {
            StackPanel::new().spacing(8.0).children(rows)
        };
        Border::new().padding(FLYOUT_PADDING).content(content)
    }
}

fn wait_for_level_change(display: &DisplayStore, context: &ComponentContext<Brightness>) {
    let display = display.clone();
    context.spawn_background(
        move |_| match display.level_events().lock().unwrap().recv() {
            Ok(()) => {
                let (names, levels) = display.levels_and_names();
                Message::Synced(Some((names, levels)))
            }
            Err(_) => Message::Synced(None),
        },
    );
}

fn monitor_icon() -> View {
    FontIcon::new().glyph("\u{E7F4}").into()
}

fn install_flyout_window(display: &DisplayStore, window: &WindowStore, hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);

        let ex_style = GetWindowLongPtrW(hwnd, GWLP_EXSTYLE);
        let _ = SetWindowLongPtrW(hwnd, GWLP_EXSTYLE, ex_style | WS_EX_TOOLWINDOW.0 as isize);

        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const c_void,
            size_of_val(&preference) as u32,
        );

        window.publish_flyout(hwnd.0 as isize);
        let ref_data = Arc::into_raw(Arc::clone(&window.0)) as usize;
        let installed =
            SetWindowSubclass(hwnd, Some(flyout_hook_proc), FLYOUT_SUBCLASS_ID, ref_data).as_bool();
        if !installed {
            drop(Arc::from_raw(ref_data as *const WindowStoreInner));
            return;
        }

        show(display, window, hwnd);
    }
}

unsafe extern "system" fn flyout_hook_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    ref_data: usize,
) -> LRESULT {
    if message == WM_ACTIVATE && ref_data != 0 {
        if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
            let store = unsafe { &*(ref_data as *const WindowStoreInner) };
            hide(store, hwnd);
        }
    } else if message == WM_NCDESTROY && ref_data != 0 {
        let store = unsafe { Arc::from_raw(ref_data as *const WindowStoreInner) };
        cancel_flyout_anim(&store);
        store.retire_flyout(hwnd.0 as isize);
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(flyout_hook_proc), FLYOUT_SUBCLASS_ID);
        }
        drop(store);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

pub(crate) fn show(display: &DisplayStore, window: &WindowStore, hwnd: HWND) {
    display.kick_level_sync();
    settle_client_size(display, hwnd);
    position_flyout(hwnd);
    let (left, top, height) = unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        (rect.left, rect.top, rect.bottom - rect.top)
    };
    let margin = unsafe { GetDpiForWindow(hwnd) as i32 * 12 / 96 };
    let from_y = top + height + margin;
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            left,
            from_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    start_flyout_anim(&window.0, hwnd, left, from_y, top, None);
}

pub(crate) fn hide(store: &WindowStoreInner, hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    let (left, top, height) = unsafe {
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        (rect.left, rect.top, rect.bottom - rect.top)
    };
    let margin = unsafe { GetDpiForWindow(hwnd) as i32 * 12 / 96 };
    let hwnd_addr = hwnd.0 as isize;
    let hidden_at = store.flyout_hidden_at();
    start_flyout_anim(
        store,
        hwnd,
        left,
        top,
        top + height + margin,
        Some(Box::new(move || {
            let hwnd = HWND(hwnd_addr as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            *hidden_at.lock().unwrap() = Some(Instant::now());
        })),
    );
}

pub(crate) struct FlyoutAnim {
    cancel: Arc<AtomicBool>,
    on_complete: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

fn start_flyout_anim(
    store: &WindowStoreInner,
    hwnd: HWND,
    x: i32,
    from_y: i32,
    to_y: i32,
    on_complete: Option<Box<dyn FnOnce() + Send>>,
) {
    cancel_flyout_anim(store);
    let anim = Arc::new(FlyoutAnim {
        cancel: Arc::new(AtomicBool::new(false)),
        on_complete: Mutex::new(on_complete),
    });
    store.set_flyout_anim(Arc::clone(&anim));
    let result = thread::Builder::new().name("flyout-anim".into()).spawn({
        let hwnd = hwnd.0 as usize;
        move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let hwnd = HWND(hwnd as *mut _);
            let Ok(manager) = Manager::new() else {
                return;
            };
            let Ok(library) = TransitionLibrary::new() else {
                return;
            };
            let Ok(variable) = manager.create_variable(0.0) else {
                return;
            };
            let Ok(transition) =
                library.accelerate_decelerate(FLYOUT_ANIM_DURATION.as_secs_f64(), 1.0, 0.1, 0.6)
            else {
                return;
            };
            let Ok(storyboard) = manager.create_storyboard() else {
                return;
            };
            if storyboard.add_transition(&variable, &transition).is_err() {
                return;
            }
            if storyboard.schedule(0.0).is_err() {
                return;
            }

            let started = Instant::now();
            loop {
                if anim.cancel.load(Ordering::Acquire) {
                    break;
                }
                unsafe {
                    let _ = DwmFlush();
                }
                let elapsed = started.elapsed().as_secs_f64();
                if manager.update(elapsed).is_err() {
                    break;
                }
                let Ok(progress) = variable.value() else {
                    break;
                };
                let y = from_y as f64 + (to_y - from_y) as f64 * progress;
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        x,
                        y as i32,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                    );
                }
                if elapsed >= FLYOUT_ANIM_DURATION.as_secs_f64() {
                    break;
                }
            }
            if !anim.cancel.load(Ordering::Acquire) {
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        to_y,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                    );
                }
                if let Some(on_complete) = anim.on_complete.lock().unwrap().take() {
                    on_complete();
                }
            }
        }
    });
    if let Err(error) = result {
        eprintln!("flyout animation thread failed: {error}");
        cancel_flyout_anim(store);
    }
}

fn cancel_flyout_anim(store: &WindowStoreInner) {
    if let Some(anim) = store.take_flyout_anim() {
        anim.cancel.store(true, Ordering::Release);
    }
}

fn settle_client_size(display: &DisplayStore, hwnd: HWND) {
    unsafe {
        let count = display.monitor_count();
        let height_dips = FLYOUT_BASE_HEIGHT + FLYOUT_ROW_HEIGHT * count;
        let dpi = f64::from(GetDpiForWindow(hwnd));
        let target = |dips: f64| (dips * dpi / 96.0).round() as i32;
        let mut client = RECT::default();
        let mut outer = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(hwnd, &mut outer);
        let (outer_w, outer_h) = (outer.right - outer.left, outer.bottom - outer.top);
        let want_w = target(FLYOUT_WIDTH) + (outer_w - (client.right - client.left));
        let want_h = target(height_dips) + (outer_h - (client.bottom - client.top));
        if outer_w != want_w || outer_h != want_h {
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                want_w,
                want_h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

fn position_flyout(hwnd: HWND) {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let work_area = info.rcWork;
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let margin = GetDpiForWindow(hwnd) as i32 * 12 / 96;
        let x = work_area.right - (rect.right - rect.left) - margin;
        let y = work_area.bottom - (rect.bottom - rect.top) - margin;
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
