use std::rc::Rc;

use tray_icon_win::TrayEvent;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, IsWindowVisible, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        SetWindowLongPtrW, SetWindowPos, ShowWindow, WHEEL_DELTA, WS_EX_TOOLWINDOW,
    },
};
use windows_reactor::*;

use crate::{
    auto_start, flyout,
    store::{
        display::DisplayStore, prefs::PrefsStore, tray::TrayStore, window::WindowStore,
    },
    win32::GWLP_EXSTYLE,
};

pub(crate) const BRIGHTNESS_WHEEL_STEP: f64 = 5.0;

const MENU_AUTOSTART: &str = "开机自动启动";
const MENU_RESTORE_BRIGHTNESS: &str = "启动时恢复亮度";
const MENU_EXIT: &str = "退出";

#[derive(Clone)]
pub(crate) struct TrayHostInput {
    pub(crate) app: Rc<AppContext>,
    pub(crate) display: DisplayStore,
    pub(crate) prefs: PrefsStore,
    pub(crate) tray: TrayStore,
    pub(crate) window: WindowStore,
}

impl PartialEq for TrayHostInput {
    fn eq(&self, other: &Self) -> bool {
        self.display == other.display
            && self.prefs == other.prefs
            && self.tray == other.tray
            && self.window == other.window
            && Rc::ptr_eq(&self.app, &other.app)
    }
}

#[derive(Clone, PartialEq)]
pub(crate) enum Message {
    WindowReady(isize),
    BrightnessSynced(f64),
    Tray(TrayEvent),
    Menu(String),
    TrayGone,
}

pub(crate) struct TrayHost {
    app: Rc<AppContext>,
    display: DisplayStore,
    prefs: PrefsStore,
    tray: TrayStore,
    window: WindowStore,
}

impl Component for TrayHost {
    type Input = TrayHostInput;
    type Message = Message;

    fn create(input: &Self::Input, context: &ComponentContext<Self>) -> Self {
        wait_for_tray_event(&input.tray, context);
        wait_for_brightness(&input.display, context);
        if !context.run_window(|window| Message::WindowReady(window.as_raw() as isize)) {
            eprintln!("tray_host: 控制器窗口句柄获取未排上");
        }
        input.tray.sync();
        Self {
            app: Rc::clone(&input.app),
            display: input.display.clone(),
            prefs: input.prefs.clone(),
            tray: input.tray.clone(),
            window: input.window.clone(),
        }
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::WindowReady(raw) => {
                let hwnd = HWND(raw as *mut _);
                unsafe {
                    let ex_style = GetWindowLongPtrW(hwnd, GWLP_EXSTYLE);
                    let _ = SetWindowLongPtrW(
                        hwnd,
                        GWLP_EXSTYLE,
                        ex_style | WS_EX_TOOLWINDOW.0 as isize,
                    );
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        1,
                        1,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            Message::BrightnessSynced(level) => {
                self.display.set_brightness(level);
                self.tray.sync();
                self.prefs.set(|settings| settings.brightness = level);
                wait_for_brightness(&self.display, context);
            }
            Message::Tray(event) => {
                self.handle_tray_event(event, context);
                wait_for_tray_event(&self.tray, context);
            }
            Message::Menu(choice) => self.handle_menu_choice(&choice),
            Message::TrayGone => self.exit_app(),
        }
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_visuals(WindowVisuals::new().client_size(1.0, 1.0));
        View::empty()
    }
}

impl TrayHost {
    fn handle_tray_event(&mut self, event: TrayEvent, context: &ComponentContext<Self>) {
        match event {
            TrayEvent::Activated { .. } => self.toggle_flyout(context),
            TrayEvent::Wheel(delta) => self.adjust_brightness(delta),
            TrayEvent::ThemeChanged(_) => {}
            TrayEvent::ContextMenu { position } => self.show_menu(position, context),
        }
    }

    fn adjust_brightness(&mut self, delta: i16) {
        if delta == 0 {
            return;
        }
        let steps = f64::from(delta) / f64::from(WHEEL_DELTA) * BRIGHTNESS_WHEEL_STEP;
        self.display.adjust_all(steps);
        let current = self.display.brightness();
        let next = (current + steps).clamp(0.0, 100.0);
        if next == current {
            return;
        }
        self.display.set_brightness(next);
        self.tray.sync();
    }

    fn toggle_flyout(&self, context: &ComponentContext<Self>) {
        if let Some(hwnd) = self.window.flyout_hwnd() {
            if unsafe { IsWindowVisible(hwnd) }.as_bool() {
                flyout::hide(&self.window.0, hwnd);
            } else {
                if !self.window.flyout_hidden_within(flyout::REOPEN_GRACE) {
                    flyout::show(&self.display, &self.window, hwnd);
                }
            }
        } else if self.window.claim_flyout_open() {
            let _ = context.open_window(View::component::<flyout::Brightness>(
                flyout::BrightnessInput {
                    display: self.display.clone(),
                    window: self.window.clone(),
                },
            ));
        }
    }

    fn handle_menu_choice(&self, choice: &str) {
        match choice {
            MENU_AUTOSTART => {
                let next = !auto_start::is_enabled();
                if let Err(error) = auto_start::set_enabled(next) {
                    eprintln!("切换开机自启失败：{error}");
                }
                self.tray.sync();
            }
            MENU_RESTORE_BRIGHTNESS => {
                let next = !self.prefs.snapshot().restore_brightness;
                self.prefs
                    .set(|settings| settings.restore_brightness = next);
            }
            MENU_EXIT => self.exit_app(),
            _ => {}
        }
    }

    fn show_menu(&self, position: tray_icon_win::Point, context: &ComponentContext<Self>) {
        let menu = Menu::new(
            [
                MenuItem::checkable("autostart", MENU_AUTOSTART, auto_start::is_enabled()),
                MenuItem::checkable(
                    "restore-brightness",
                    MENU_RESTORE_BRIGHTNESS,
                    self.prefs.snapshot().restore_brightness,
                ),
                MenuItem::separator("sep-exit"),
                MenuItem::item("exit", MENU_EXIT),
            ],
            context
                .sender()
                .callback(|label: String| Message::Menu(label)),
        );
        if let Err(error) = self
            .app
            .show_menu_at(ScreenPoint::new(position.x, position.y), menu)
        {
            eprintln!("托盘右键菜单弹出失败：{error}");
        }
    }

    fn exit_app(&self) {
        self.prefs.flush_now();
        if let Err(error) = self.app.exit() {
            eprintln!("退出失败：{error}");
        }
    }
}

fn wait_for_tray_event(tray: &TrayStore, context: &ComponentContext<TrayHost>) {
    let tray = tray.clone();
    context.spawn_background(move |_| match tray.events().lock().unwrap().recv() {
        Ok(event) => Message::Tray(event),
        Err(_) => Message::TrayGone,
    });
}

fn wait_for_brightness(display: &DisplayStore, context: &ComponentContext<TrayHost>) {
    let display = display.clone();
    context.spawn_background(
        move |_| match display.brightness_events().lock().unwrap().recv() {
            Ok(level) => Message::BrightnessSynced(level),
            Err(_) => Message::TrayGone,
        },
    );
}
