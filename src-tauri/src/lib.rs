//! Local Server Manager: запуск и контроль локальных серверов разработки из трея.
//!
//! Здесь только оболочка: окно, трей и команды для интерфейса. Вся работа с процессами
//! живёт в крейте `lsm-core`.

mod commands;
mod tray;

use lsm_core::{
    config::{self, Config, Launcher, Project},
    portbook::PortBook,
    runner::{Output, ProcView, Runner, Sink, Spec},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

pub const APP_NAME: &str = "Local Server Manager";
pub const MAIN_WINDOW: &str = "main";
/// Совпадает с `identifier` в tauri.conf.json: настройки лежат рядом с данными WebView.
const DATA_FOLDER: &str = "com.localservermanager.app";
/// С этим аргументом приложение запускает автозагрузка Windows.
const AUTOSTART_ARG: &str = "--autostart";
/// Как часто заглядывать в таблицу портов, чтобы запомнить новые. Вкладка портов при открытии
/// обновляется сама и чаще: это фоновая память на случай, когда её никто не смотрит.
const PORT_WATCH: Duration = Duration::from_secs(20);

pub struct AppState {
    config: Mutex<Config>,
    pub dir: PathBuf,
    pub runner: Arc<Runner>,
    /// Ошибка чтения настроек при старте: интерфейс покажет её, когда загрузится.
    pub load_error: Option<String>,
    /// Память о портах и время, когда её последний раз писали на диск.
    pub ports: Mutex<(PortBook, Instant)>,
}

impl AppState {
    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Config {
        self.config().clone()
    }

    pub fn save(&self, cfg: &Config) -> Result<(), String> {
        config::save(&self.dir.join("config.json"), cfg)
    }

    pub fn find(&self, launcher_id: &str) -> Option<(Project, Launcher)> {
        let cfg = self.config();
        cfg.projects.iter().find_map(|p| {
            p.launchers.iter().find(|l| l.id == launcher_id).map(|l| (p.clone(), l.clone()))
        })
    }

    pub fn spec(&self, launcher_id: &str) -> Option<Spec> {
        self.find(launcher_id).map(|(_, l)| spec_of(&l))
    }

    /// Серверы в порядке списка с подписями: «SZ Learn» или «Mega Agent · web».
    pub fn labels_in_order(&self) -> Vec<(String, String)> {
        let cfg = self.config();
        cfg.projects
            .iter()
            .flat_map(|p| {
                let many = p.launchers.len() > 1;
                p.launchers.iter().map(move |l| {
                    (l.id.clone(), if many { format!("{} · {}", p.name, l.title) } else { p.name.clone() })
                })
            })
            .collect()
    }

    /// Подписи серверов по идентификатору способа запуска.
    pub fn labels(&self) -> HashMap<String, String> {
        self.labels_in_order().into_iter().collect()
    }

    pub fn ports_file(&self) -> PathBuf {
        self.dir.join("ports.json")
    }
}

pub fn spec_of(l: &Launcher) -> Spec {
    Spec { id: l.id.clone(), kind: l.kind.clone(), command: l.command.clone(), cwd: l.cwd.clone() }
}

/// Мост от ядра к окну и трею. Ядро создаётся раньше приложения, поэтому ссылку на него
/// сюда кладут позже, в `setup`.
#[derive(Default)]
struct UiSink(OnceLock<AppHandle>);

impl Sink for UiSink {
    fn procs(&self, views: Vec<ProcView>) {
        if let Some(app) = self.0.get() {
            let _ = app.emit("procs", &views);
            tray::refresh(app);
            // Порт только что поднятого сервера запоминаем сразу, не дожидаясь фонового обхода:
            // короткий запуск между двумя обходами иначе не оставил бы следа.
            if views.iter().any(|v| !v.ports.is_empty()) {
                commands::scan_ports(&app.state::<AppState>());
            }
        }
    }

    fn output(&self, batches: Vec<Output>) {
        if let Some(app) = self.0.get() {
            let _ = app.emit("term", &batches);
        }
    }

    fn crashed(&self, id: &str, code: i32) {
        let Some(app) = self.0.get() else { return };
        let state = app.state::<AppState>();
        if !state.config().settings.notify_on_crash {
            return;
        }
        if let Some(what) = state.labels().remove(id) {
            let _ = app
                .notification()
                .builder()
                .title("Сервер упал")
                .body(format!("{what}: код выхода {code}"))
                .show();
        }
    }
}

fn data_dir() -> PathBuf {
    // Переопределение нужно для проверок: поднять приложение на чистых настройках,
    // не трогая рабочий список репозиториев.
    if let Some(dir) = std::env::var_os("LSM_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(DATA_FOLDER)
}

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        tray::restore_count(app);
    }
}

/// Выход по просьбе пользователя. Работающие серверы погибнут вместе с приложением,
/// поэтому при них сначала спрашиваем: вопрос задаёт окно, а не системный диалог.
pub fn request_quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.runner.active_ids().is_empty() {
        quit(app);
    } else {
        show_main(app);
        let _ = app.emit("ask-quit", ());
    }
}

pub fn quit(app: &AppHandle) {
    app.state::<AppState>().runner.shutdown();
    app.exit(0);
}

pub fn run() {
    let dir = data_dir();
    // Папка нужна сразу: кнопка «Открыть» в настройках ведёт в неё и до первого сохранения.
    let _ = std::fs::create_dir_all(&dir);
    let (cfg, load_error) = config::load(&dir.join("config.json"));

    let sink = Arc::new(UiSink::default());
    let runner = Runner::new(sink.clone(), cfg.settings.log_lines);
    // Состояние регистрируется до запуска окна: страница может обратиться к нему
    // раньше, чем отработает `setup`.
    let ports = PortBook::load(&dir.join("ports.json"));
    let state = AppState { config: Mutex::new(cfg), dir, runner, load_error, ports: Mutex::new((ports, Instant::now())) };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Повторный запуск показывает окно уже работающего экземпляра.
            show_main(app);
        }))
        .plugin(tauri_plugin_autostart::Builder::new().args([AUTOSTART_ARG]).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .manage(tray::TrayState::default())
        .setup(move |app| {
            let handle = app.handle().clone();
            let _ = sink.0.set(handle.clone());

            let state = app.state::<AppState>();
            state.runner.spawn_ticker();
            tray::build(&handle)?;

            let from_autostart = std::env::args().any(|a| a == AUTOSTART_ARG);
            if !(from_autostart && state.config().settings.start_hidden) {
                show_main(&handle);
            }

            let watcher = handle.clone();
            std::thread::spawn(move || loop {
                commands::scan_ports(&watcher.state::<AppState>());
                std::thread::sleep(PORT_WATCH);
            });

            std::thread::spawn(move || commands::start_marked(&handle));
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != MAIN_WINDOW {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let app = window.app_handle();
                if app.state::<AppState>().config().settings.close_to_tray {
                    let _ = window.hide();
                } else {
                    request_quit(app);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::get_procs,
            commands::app_info,
            commands::scan_folder,
            commands::add_project,
            commands::update_project,
            commands::remove_project,
            commands::start_launcher,
            commands::stop_launcher,
            commands::restart_launcher,
            commands::pause_launcher,
            commands::resume_launcher,
            commands::start_all,
            commands::stop_all,
            commands::get_output,
            commands::clear_output,
            commands::term_input,
            commands::term_resize,
            commands::get_ports,
            commands::forget_port,
            commands::save_settings,
            commands::open_url,
            commands::open_folder,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить Local Server Manager");
}
