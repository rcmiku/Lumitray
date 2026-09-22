use std::{io, sync::mpsc};

use tray_icon_win::{SendIcon, Theme, TrayConfig, TrayEvent, TrayHandle};

use crate::store::tray::TrayStatus;

const TRAY_CLASS: &str = "lumitray.tray.v1";

const TRAY_ICON: &str = include_str!("../assets/ic_brightness.svg");

const LIGHT_FILL: &str = "black";
const DARK_FILL: &str = "white";

struct TrayIcons {
    brightness: [SendIcon; 2],
}

impl TrayIcons {
    fn pick(&self, theme: Theme) -> SendIcon {
        let index = match theme {
            Theme::Light => 0,
            Theme::Dark => 1,
        };
        self.brightness[index]
    }
}

fn load_tray_icons() -> io::Result<TrayIcons> {
    fn pair(svg: &str) -> io::Result<[SendIcon; 2]> {
        Ok([
            SendIcon::from_svg(svg)?,
            SendIcon::from_svg_with(svg, &[(LIGHT_FILL, DARK_FILL)])?,
        ])
    }
    Ok(TrayIcons {
        brightness: pair(TRAY_ICON)?,
    })
}

pub(crate) fn start_tray() -> io::Result<(TrayHandle<TrayStatus>, mpsc::Receiver<TrayEvent>)> {
    let icons = load_tray_icons()?;
    TrayHandle::start(TrayConfig {
        class_name: TRAY_CLASS,
        initial_status: TrayStatus::default(),
        icon_for: Box::new(move |_status: &TrayStatus, theme: Theme| icons.pick(theme)),
        tooltip_for: Box::new(|status: &TrayStatus| {
            let name = if status.name.is_empty() {
                "显示器"
            } else {
                status.name.as_str()
            };
            format!("{name} (显示器亮度) : {:.0}%", status.brightness).into()
        }),
        menu: Vec::new(),
        enable_wheel_scroll: true,
    })
}
