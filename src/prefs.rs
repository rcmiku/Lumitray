use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_RoamingAppData, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    },
    core::GUID,
};

type Job = (PathBuf, String);

const CURRENT_VERSION: u32 = 1;

fn read_only_mode(corrupt: bool, loaded_version: u32) -> bool {
    corrupt || loaded_version > CURRENT_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    pub(crate) brightness: f64,
    pub(crate) restore_brightness: bool,
    version: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            brightness: 50.0,
            restore_brightness: false,
            version: 1,
        }
    }
}

pub(crate) struct Prefs {
    settings: Mutex<Settings>,
    path: Option<PathBuf>,
    read_only: bool,
    save_tx: mpsc::Sender<Job>,
    write_lock: Arc<Mutex<()>>,
    shutdown: Arc<AtomicBool>,
}

impl Prefs {
    pub(crate) fn load() -> Self {
        let path = config_path();
        let (settings, corrupt) = match &path {
            Some(path) => match fs::read_to_string(path) {
                Ok(content) => match toml::from_str::<Settings>(&content) {
                    Ok(settings) => (settings, false),
                    Err(error) => {
                        eprintln!(
                            "prefs: 解析 {} 失败（{error}），本次使用默认值",
                            path.display()
                        );
                        (Settings::default(), true)
                    }
                },
                Err(_) => {
                    let settings = Settings::default();
                    match toml::to_string_pretty(&settings) {
                        Ok(content) => {
                            if let Err(error) = write_atomic(path, &content) {
                                eprintln!("prefs: 创建 {} 失败：{error}", path.display());
                            }
                        }
                        Err(error) => eprintln!("prefs: 序列化默认值失败：{error}"),
                    }
                    (settings, false)
                }
            },
            None => (Settings::default(), false),
        };
        if settings.version > CURRENT_VERSION {
            eprintln!(
                "prefs: 配置版本 {} 高于本程序认识的 {CURRENT_VERSION}，本次只读——\
                 不用旧结构覆盖你的文件",
                settings.version
            );
        }
        let read_only = read_only_mode(corrupt, settings.version);

        let (save_tx, save_rx) = mpsc::channel::<Job>();
        let write_lock = Arc::new(Mutex::new(()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_lock = Arc::clone(&write_lock);
        let worker_shutdown = Arc::clone(&shutdown);
        let _ = thread::Builder::new()
            .name("prefs-save".into())
            .spawn(move || {
                while let Ok(mut job) = save_rx.recv() {
                    while let Ok(next) = save_rx.try_recv() {
                        job = next;
                    }
                    let guard = worker_lock.lock().unwrap();
                    if worker_shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    if let Err(error) = write_atomic(&job.0, &job.1) {
                        eprintln!("prefs: 保存 {} 失败：{error}", job.0.display());
                    }
                    drop(guard);
                    thread::sleep(Duration::from_millis(50));
                }
            });

        Self {
            settings: Mutex::new(settings),
            path,
            read_only,
            save_tx,
            write_lock,
            shutdown,
        }
    }

    pub(crate) fn snapshot(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub(crate) fn set(&self, apply: impl FnOnce(&mut Settings)) {
        {
            let mut settings = self.settings.lock().unwrap();
            apply(&mut settings);
            settings.brightness = settings.brightness.clamp(0.0, 100.0);
        }
        self.save_soon();
    }

    fn save_soon(&self) {
        if self.read_only {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        match toml::to_string_pretty(&*self.settings.lock().unwrap()) {
            Ok(content) => {
                let _ = self.save_tx.send((path.clone(), content));
            }
            Err(error) => eprintln!("prefs: 序列化失败：{error}"),
        }
    }

    pub(crate) fn flush_now(&self) {
        if self.read_only {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        self.shutdown.store(true, Ordering::Release);
        let _guard = self.write_lock.lock().unwrap();
        let Ok(content) = toml::to_string_pretty(&*self.settings.lock().unwrap()) else {
            return;
        };
        if let Err(error) = write_atomic(path, &content) {
            eprintln!("prefs: 刷写 {} 失败：{error}", path.display());
        }
    }
}

fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

pub(crate) fn data_dir() -> Option<PathBuf> {
    let roaming = roaming_app_data()?;
    let dir = roaming.join("lumitray");
    if let Err(error) = fs::create_dir_all(&dir) {
        eprintln!("prefs: 创建 {} 失败：{error}", dir.display());
        return None;
    }
    Some(dir)
}

fn config_path() -> Option<PathBuf> {
    Some(data_dir()?.join("config.toml"))
}

fn roaming_app_data() -> Option<PathBuf> {
    known_folder(&FOLDERID_RoamingAppData)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        match SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None) {
            Ok(path) => {
                let string = path.to_string().ok();
                CoTaskMemFree(Some(path.as_ptr().cast()));
                string.map(PathBuf::from)
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_missing_fields_loads() {
        let settings: Settings = toml::from_str("brightness = 42.0\nversion = 1\n").unwrap();
        assert_eq!(settings.brightness, 42.0);
        assert!(!settings.restore_brightness);
    }

    #[test]
    fn restore_brightness_round_trips() {
        let text = toml::to_string_pretty(&Settings {
            restore_brightness: false,
            ..Settings::default()
        })
        .unwrap();
        assert!(
            text.contains("restore_brightness = false"),
            "落盘内容：{text}"
        );
        let back: Settings = toml::from_str(&text).unwrap();
        assert!(!back.restore_brightness);
    }

    #[test]
    fn read_only_mode_covers_corrupt_and_future_versions() {
        assert!(!read_only_mode(false, CURRENT_VERSION));
        assert!(!read_only_mode(false, 0));
        assert!(read_only_mode(true, CURRENT_VERSION));
        assert!(read_only_mode(false, CURRENT_VERSION + 1));
    }

    #[test]
    fn snapshot_twice_in_one_expression_does_not_deadlock() {
        let prefs = Prefs::in_memory();
        let (brightness, restore) = (
            prefs.snapshot().brightness,
            prefs.snapshot().restore_brightness,
        );
        assert_eq!(brightness, 50.0);
        assert!(!restore);
    }

    impl Prefs {
        fn in_memory() -> Self {
            let (save_tx, _rx) = mpsc::channel();
            Self {
                settings: Mutex::new(Settings::default()),
                path: None,
                read_only: false,
                save_tx,
                write_lock: Arc::new(Mutex::new(())),
                shutdown: Arc::new(AtomicBool::new(false)),
            }
        }
    }
}
