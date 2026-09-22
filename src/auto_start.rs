use windows::core::HSTRING;
use windows::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ, REG_SAM_FLAGS,
    },
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE_NAME: &str = "lumitray";

pub(crate) fn is_enabled() -> bool {
    run_value_exists() && !startup_disabled()
}

pub(crate) fn set_enabled(enable: bool) -> Result<(), String> {
    let Some(command) = command_line() else {
        return Err("无法获取当前 exe 路径".to_string());
    };
    let mut hkey = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(RUN_KEY),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("打开 Run 键失败（{status:?}）"));
    }
    let result = if enable {
        let write = unsafe {
            RegSetValueExW(
                hkey,
                &HSTRING::from(VALUE_NAME),
                None,
                REG_SZ,
                Some(&command),
            )
        };
        if write == ERROR_SUCCESS
            && let Some(approved) = open(APPROVED_KEY, KEY_SET_VALUE)
        {
            unsafe {
                let _ = RegDeleteValueW(approved, &HSTRING::from(VALUE_NAME));
                let _ = RegCloseKey(approved);
            }
        }
        write
    } else {
        match unsafe { RegDeleteValueW(hkey, &HSTRING::from(VALUE_NAME)) } {
            ERROR_SUCCESS => ERROR_SUCCESS,
            ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
            status => status,
        }
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("写入自启状态失败（{result:?}）"))
    }
}

pub(crate) fn refresh() {
    if !run_value_exists() {
        return;
    }
    let _ = set_enabled(true);
}

fn run_value_exists() -> bool {
    let Some(hkey) = open(RUN_KEY, KEY_QUERY_VALUE) else {
        return false;
    };
    let status =
        unsafe { RegQueryValueExW(hkey, &HSTRING::from(VALUE_NAME), None, None, None, None) };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    status == ERROR_SUCCESS
}

fn startup_disabled() -> bool {
    let Some(hkey) = open(APPROVED_KEY, KEY_QUERY_VALUE) else {
        return false;
    };
    let mut data = [0u8; 32];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            &HSTRING::from(VALUE_NAME),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    status == ERROR_SUCCESS && size >= 1 && (data[0] & 0x01) == 1
}

fn open(subkey: &str, access: REG_SAM_FLAGS) -> Option<HKEY> {
    let mut hkey = HKEY::default();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(subkey), None, access, &mut hkey) };
    (status == ERROR_SUCCESS).then_some(hkey)
}

fn command_line() -> Option<Vec<u8>> {
    let exe = std::env::current_exe().ok()?;
    let mut bytes = Vec::new();
    for unit in format!("\"{}\"", exe.display()).encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    Some(bytes)
}
