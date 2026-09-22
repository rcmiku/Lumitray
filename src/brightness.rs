use std::{
    fmt,
    mem::size_of,
    ptr,
    sync::{
        Arc, Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Devices::Display::{
        DestroyPhysicalMonitor, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
        GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
        GetVCPFeatureAndVCPFeatureReply, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS,
        QueryDisplayConfig, SetVCPFeature,
    },
    Foundation::{HANDLE, LPARAM, RECT, WIN32_ERROR},
    Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    },
};
use windows::core::{BOOL, Result};

const VCP_LUMINANCE: u8 = 0x10;
const IDLE_REENUMERATE: Duration = Duration::from_secs(15);
const REENUMERATE_COOLDOWN: Duration = Duration::from_secs(5);
const APPLY_RETRIES: u32 = 3;
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) enum Command {
    SetAll(f64),
    Refresh,
    Set(usize, f64),
    AdjustAll(f64),
}

#[derive(Clone, Default)]
pub(crate) struct MonitorStatus {
    pub(crate) names: Vec<String>,
    pub(crate) levels: Vec<f64>,
}

pub(crate) fn start_worker(
    status: Arc<Mutex<MonitorStatus>>,
    level_notify: mpsc::Sender<()>,
    tray_notify: mpsc::Sender<f64>,
) -> mpsc::Sender<Command> {
    let (tx, rx) = mpsc::channel::<Command>();
    let _ = thread::Builder::new().name("brightness".into()).spawn(move || {
        let mut monitors: Vec<Monitor> = Vec::new();
        let mut levels: Vec<f64> = Vec::new();
        let mut last_enumerate: Option<Instant> = None;
        loop {
            let first = match rx.recv_timeout(IDLE_REENUMERATE) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    if last_enumerate.is_none_or(|at| at.elapsed() >= REENUMERATE_COOLDOWN) {
                        monitors = enumerate();
                        last_enumerate = Some(Instant::now());
                        align_levels(&monitors, &mut levels);
                        publish(&status, &level_notify, &tray_notify, &monitors, &levels);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut commands = vec![first];
            while let Ok(next) = rx.try_recv() {
                let merged = match (commands.last_mut(), &next) {
                    (Some(Command::Set(last, existing)), Command::Set(next_index, latest))
                        if last == next_index =>
                    {
                        *existing = *latest;
                        true
                    }
                    (Some(Command::AdjustAll(existing)), Command::AdjustAll(delta)) => {
                        *existing += *delta;
                        true
                    }
                    (Some(Command::SetAll(existing)), Command::SetAll(latest)) => {
                        *existing = *latest;
                        true
                    }
                    _ => false,
                };
                if !merged {
                    commands.push(next);
                }
            }
            for command in commands {
                let enumerate_now = match last_enumerate {
                    Some(at) => monitors.is_empty() && at.elapsed() >= REENUMERATE_COOLDOWN,
                    None => true,
                };
                if enumerate_now {
                    monitors = enumerate();
                    last_enumerate = Some(Instant::now());
                }
                align_levels(&monitors, &mut levels);
                match command {
                    Command::SetAll(value) => {
                        levels.iter_mut().for_each(|level| *level = value);
                        publish(&status, &level_notify, &tray_notify, &monitors, &levels);
                        let mut rounds = 0;
                        loop {
                            rounds += 1;
                            let mut any_written = false;
                            for monitor in &monitors {
                                if monitor.set_luminance(value).is_ok() {
                                    any_written = true;
                                }
                            }
                            if any_written || rounds >= APPLY_RETRIES {
                                break;
                            }
                            thread::sleep(RETRY_INTERVAL);
                            monitors = enumerate();
                            last_enumerate = Some(Instant::now());
                            align_levels(&monitors, &mut levels);
                            levels.iter_mut().for_each(|level| *level = value);
                        }
                    }
                    Command::Refresh => {
                        levels = monitors.iter().map(|monitor| monitor.level).collect();
                        publish(&status, &level_notify, &tray_notify, &monitors, &levels);
                    }
                    Command::Set(index, value) => {
                        if let (Some(monitor), Some(level)) =
                            (monitors.get(index), levels.get_mut(index))
                        {
                            *level = value;
                            publish(&status, &level_notify, &tray_notify, &monitors, &levels);
                            let _ = monitor.set_luminance(value);
                        }
                    }
                    Command::AdjustAll(delta) => {
                        for level in levels.iter_mut() {
                            *level = (*level + delta).clamp(0.0, 100.0);
                        }
                        publish(&status, &level_notify, &tray_notify, &monitors, &levels);
                        for (level, monitor) in levels.iter().zip(&monitors) {
                            let _ = monitor.set_luminance(*level);
                        }
                    }
                }
            }
        }
    });
    tx
}

fn align_levels(monitors: &[Monitor], levels: &mut Vec<f64>) {
    if levels.len() != monitors.len() {
        *levels = monitors.iter().map(|monitor| monitor.level).collect();
    }
}

fn publish(
    status: &Mutex<MonitorStatus>,
    notify: &mpsc::Sender<()>,
    tray_notify: &mpsc::Sender<f64>,
    monitors: &[Monitor],
    levels: &[f64],
) {
    {
        let mut slot = status.lock().unwrap();
        slot.names = monitors.iter().map(|m| m.description.clone()).collect();
        slot.levels = levels.to_vec();
    }
    let _ = notify.send(());
    if let Some(level) = levels.first() {
        let _ = tray_notify.send(*level);
    }
}

struct Monitor {
    handle: HANDLE,
    description: String,
    max_luminance: u32,
    level: f64,
}

impl Monitor {
    fn probe(handle: HANDLE, description: String) -> Option<Self> {
        match luminance(handle) {
            Ok((current, max)) if max > 0 => Some(Self {
                handle,
                description,
                max_luminance: max,
                level: f64::from(current) / f64::from(max) * 100.0,
            }),
            _ => {
                unsafe {
                    let _ = DestroyPhysicalMonitor(handle);
                }
                None
            }
        }
    }

    fn set_luminance(&self, value: f64) -> Result<()> {
        let max = f64::from(self.max_luminance);
        let target = (value / 100.0 * max).round().clamp(0.0, max) as u32;
        BOOL(unsafe { SetVCPFeature(self.handle, VCP_LUMINANCE, target) }).ok()
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyPhysicalMonitor(self.handle);
        }
    }
}

impl fmt::Debug for Monitor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Monitor")
            .field("description", &self.description)
            .field("max_luminance", &self.max_luminance)
            .finish()
    }
}

fn luminance(handle: HANDLE) -> Result<(u32, u32)> {
    let mut current = 0u32;
    let mut max = 0u32;
    BOOL(unsafe {
        GetVCPFeatureAndVCPFeatureReply(handle, VCP_LUMINANCE, None, &mut current, Some(&mut max))
    })
    .ok()?;
    Ok((current, max))
}

fn enumerate() -> Vec<Monitor> {
    let friendly = friendly_names();
    let mut hmonitors = Vec::<HMONITOR>::new();
    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let list = data.0 as *mut Vec<HMONITOR>;
        unsafe {
            (*list).push(monitor);
        }
        true.into()
    }
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(ptr::addr_of_mut!(hmonitors) as _),
        );
    }
    let mut monitors = Vec::new();
    for hmonitor in hmonitors {
        monitors.extend(physical_monitors(hmonitor, &friendly));
    }
    monitors
}

fn physical_monitors(hmonitor: HMONITOR, friendly: &[(String, String)]) -> Vec<Monitor> {
    let mut count = 0u32;
    unsafe {
        if GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count).is_err() {
            return Vec::new();
        }
        let mut raw = vec![PHYSICAL_MONITOR::default(); count as usize];
        if GetPhysicalMonitorsFromHMONITOR(hmonitor, &mut raw).is_err() {
            return Vec::new();
        }

        let friendly = gdi_device_name(hmonitor).and_then(|gdi| {
            friendly
                .iter()
                .find(|(device, _)| *device == gdi)
                .map(|(_, name)| name.clone())
        });
        raw.into_iter()
            .filter_map(|item| probe(item, friendly.clone()))
            .collect()
    }
}

fn gdi_device_name(hmonitor: HMONITOR) -> Option<String> {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    if unsafe {
        GetMonitorInfoW(
            hmonitor,
            &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    }
    .as_bool()
    {
        Some(wide_string(&info.szDevice))
    } else {
        None
    }
}

fn friendly_names() -> Vec<(String, String)> {
    use windows::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    };
    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != WIN32_ERROR(0)
        {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != WIN32_ERROR(0)
        {
            return Vec::new();
        }
        paths[..path_count as usize]
            .iter()
            .filter_map(|path| {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                    return None;
                }
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: path.targetInfo.adapterId,
                        id: path.targetInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                    return None;
                }
                Some((
                    wide_string(&source.viewGdiDeviceName),
                    wide_string(&target.monitorFriendlyDeviceName),
                ))
            })
            .collect()
    }
}

fn probe(raw: PHYSICAL_MONITOR, friendly: Option<String>) -> Option<Monitor> {
    let description = friendly
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| description(&raw));
    Monitor::probe(raw.hPhysicalMonitor, description)
}

fn description(raw: &PHYSICAL_MONITOR) -> String {
    let desc = unsafe { ptr::addr_of!(raw.szPhysicalMonitorDescription).read_unaligned() };
    wide_string(&desc)
}

fn wide_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
