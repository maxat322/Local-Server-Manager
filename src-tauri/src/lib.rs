//! Local Server Manager: запуск и контроль локальных серверов разработки из трея.
//!
//! Здесь только оболочка: окно, трей и команды для интерфейса. Вся работа с процессами
//! живёт в крейте `lsm-core`.

mod commands;
mod tray;

use lsm_core::{
    config::{self, Config, Launcher, Project},
    foreign::{self, Found, Target},
    portbook::PortBook,
    runner::{Output, ProcView, Runner, Sink, Spec},
    usage::{self, Sampler, Usage},
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
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
/// Как часто замерять расход процессора, памяти и видеокарты, пока окно на экране.
const USAGE_EVERY: Duration = Duration::from_secs(2);
/// Поиск запусков мимо менеджера идёт через столько замеров: он дороже, а спешить ему некуда.
const FOREIGN_EVERY_TICKS: u32 = 2;

/// Расход ресурсов по репозиториям: событие `usage` для главной.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageReport {
    total_mem: u64,
    projects: HashMap<String, Usage>,
}

pub struct AppState {
    config: Mutex<Config>,
    pub dir: PathBuf,
    pub runner: Arc<Runner>,
    /// Ошибка чтения настроек при старте: интерфейс покажет её, когда загрузится.
    pub load_error: Option<String>,
    /// Память о портах и время, когда её последний раз писали на диск.
    pub ports: Mutex<(PortBook, Instant)>,
    /// Серверы из списка, запущенные мимо менеджера: по последнему поиску.
    foreign: Mutex<Vec<Found>>,
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

    /// Задания живых серверов с репозиторием каждого: расход считается на репозиторий целиком.
    fn jobs_by_project(&self) -> Vec<(String, Arc<lsm_core::job::Job>)> {
        let project_of: HashMap<String, String> = {
            let cfg = self.config();
            cfg.projects.iter().flat_map(|p| p.launchers.iter().map(|l| (l.id.clone(), p.id.clone()))).collect()
        };
        self.runner
            .live_jobs()
            .into_iter()
            .filter_map(|(id, job)| Some((project_of.get(&id)?.clone(), job)))
            .collect()
    }

    /// Процессы серверов, запущенных мимо менеджера, по репозиториям: их расход тоже показывается.
    fn foreign_by_project(&self) -> Vec<(String, Vec<u32>)> {
        let project_of: HashMap<String, String> = {
            let cfg = self.config();
            cfg.projects.iter().flat_map(|p| p.launchers.iter().map(|l| (l.id.clone(), p.id.clone()))).collect()
        };
        self.foreign()
            .into_iter()
            .filter_map(|f| Some((project_of.get(&f.id)?.clone(), f.pids)))
            .collect()
    }

    /// Ищет заново серверы из списка, запущенные мимо менеджера. Ищутся только те, что сейчас
    /// не работают в самом менеджере. Возвращает, изменился ли список.
    pub fn refresh_foreign(&self) -> bool {
        let alive: HashSet<String> = self.runner.active_ids().into_iter().collect();
        let targets: Vec<Target> = {
            let cfg = self.config();
            cfg.projects
                .iter()
                .flat_map(|p| {
                    p.launchers
                        .iter()
                        .filter(|l| !alive.contains(&l.id))
                        .map(|l| Target { id: l.id.clone(), paths: vec![l.cwd.clone(), p.path.clone()] })
                })
                .collect()
        };
        let exclude: HashSet<u32> = self.runner.owners().into_keys().collect();
        let found = foreign::detect(&targets, &exclude);
        let mut cache = self.foreign.lock().unwrap_or_else(|e| e.into_inner());
        if *cache == found {
            return false;
        }
        *cache = found;
        true
    }

    pub fn foreign(&self) -> Vec<Found> {
        self.foreign.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    let state = AppState {
        config: Mutex::new(cfg),
        dir,
        runner,
        load_error,
        ports: Mutex::new((ports, Instant::now())),
        foreign: Mutex::new(Vec::new()),
    };

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

            let meter = handle.clone();
            std::thread::spawn(move || {
                let mut sampler = Sampler::new();
                let total_mem = usage::total_memory();
                let mut tick: u32 = 0;
                // Что окно уже знает о запусках мимо менеджера. Сравниваем с ним, а не с прошлым
                // поиском: искать могут и команды (например, «запустить все»), не сообщая окну.
                let mut known: Vec<Found> = Vec::new();
                loop {
                    std::thread::sleep(USAGE_EVERY);
                    // В трее цифры никто не видит: замеры не должны стоить ресурсов сами.
                    let shown = meter.get_webview_window(MAIN_WINDOW).is_some_and(|w| {
                        w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
                    });
                    if !shown {
                        continue;
                    }
                    let state = meter.state::<AppState>();
                    let jobs = state.jobs_by_project();
                    let loose = state.foreign_by_project();
                    let _ = meter.emit("usage", &UsageReport { total_mem, projects: sampler.sample(&jobs, &loose) });
                    tick = tick.wrapping_add(1);
                    if tick % FOREIGN_EVERY_TICKS == 0 {
                        state.refresh_foreign();
                        let now = state.foreign();
                        if now != known {
                            let _ = meter.emit("foreign", &now);
                            known = now;
                        }
                    }
                }
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
            commands::get_foreign,
            commands::stop_foreign,
            commands::restart_foreign,
            commands::forget_port,
            commands::save_settings,
            commands::open_url,
            commands::open_folder,
            commands::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить Local Server Manager");
}
