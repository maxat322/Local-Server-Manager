//! Команды, которые вызывает интерфейс.
//!
//! Всё, что может ждать процесс или диск, помечено `async`: обычная команда выполняется
//! в главном потоке и на время ожидания замораживает окно.

use crate::{spec_of, tray, AppState};
use lsm_core::{
    config::{same_path, Config, Project, Settings},
    portbook::{worth_remembering, Sighting},
    ports,
    runner::{now_ms, Chunk, ProcView, Status},
    scan::{self, ScanResult},
    text::capitalize,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    data_dir: String,
    load_error: Option<String>,
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.snapshot()
}

#[tauri::command]
pub fn get_procs(state: State<'_, AppState>) -> Vec<ProcView> {
    state.runner.snapshot()
}

#[tauri::command]
pub fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        data_dir: state.dir.to_string_lossy().into_owned(),
        load_error: state.load_error.clone(),
    }
}

#[tauri::command(async)]
pub fn scan_folder(path: String) -> Result<ScanResult, String> {
    scan::scan(Path::new(path.trim()))
}

#[tauri::command(async)]
pub fn add_project(app: AppHandle, state: State<'_, AppState>, mut project: Project) -> Result<Config, String> {
    // Идентификаторы выдаёт ядро: запись из интерфейса может прийти с чужими или повторными.
    project.id.clear();
    for l in &mut project.launchers {
        l.id.clear();
    }
    project.normalize()?;

    let saved = {
        let mut cfg = state.config();
        if cfg.projects.iter().any(|p| same_path(&p.path, &project.path)) {
            return Err("Этот репозиторий уже есть в списке".into());
        }
        cfg.projects.push(project);
        state.save(&cfg)?;
        cfg.clone()
    };
    tray::refresh(&app);
    Ok(saved)
}

#[tauri::command(async)]
pub fn update_project(app: AppHandle, state: State<'_, AppState>, mut project: Project) -> Result<Config, String> {
    project.normalize()?;

    let (saved, removed) = {
        let mut cfg = state.config();
        let Some(slot) = cfg.projects.iter_mut().find(|p| p.id == project.id) else {
            return Err("Репозиторий не найден: возможно, его уже убрали из списка".into());
        };
        let removed: Vec<String> = slot
            .launchers
            .iter()
            .filter(|old| !project.launchers.iter().any(|l| l.id == old.id))
            .map(|old| old.id.clone())
            .collect();
        *slot = project;
        state.save(&cfg)?;
        (cfg.clone(), removed)
    };
    // Убранный способ запуска не должен продолжать работать невидимкой.
    for id in removed {
        state.runner.forget(&id);
    }
    tray::refresh(&app);
    Ok(saved)
}

#[tauri::command(async)]
pub fn remove_project(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<Config, String> {
    let (saved, removed) = {
        let mut cfg = state.config();
        let Some(index) = cfg.projects.iter().position(|p| p.id == id) else {
            return Ok(cfg.clone());
        };
        let project = cfg.projects.remove(index);
        state.save(&cfg)?;
        (cfg.clone(), project.launchers)
    };
    for l in removed {
        state.runner.forget(&l.id);
    }
    tray::refresh(&app);
    Ok(saved)
}

#[tauri::command(async)]
pub fn start_launcher(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let spec = state.spec(&id).ok_or("Способ запуска не найден")?;
    state.runner.start(&spec).map_err(capitalize)
}

#[tauri::command(async)]
pub fn stop_launcher(state: State<'_, AppState>, id: String) {
    state.runner.stop(&id);
}

#[tauri::command(async)]
pub fn restart_launcher(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let spec = state.spec(&id).ok_or("Способ запуска не найден")?;
    state.runner.restart(&spec).map_err(capitalize)
}

/// Пауза: процессы сервера замораживаются и перестают тратить процессор, но остаются в памяти.
#[tauri::command(async)]
pub fn pause_launcher(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.runner.pause(&id).map_err(capitalize)
}

#[tauri::command(async)]
pub fn resume_launcher(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.runner.resume(&id).map_err(capitalize)
}

/// Запускает всё, что ещё не работает. Возвращает число серверов, которые запустить не удалось:
/// подробности каждого сбоя уже лежат в его журнале.
#[tauri::command(async)]
pub fn start_all(state: State<'_, AppState>) -> usize {
    start_where(&state, |_| true)
}

#[tauri::command(async)]
pub fn stop_all(state: State<'_, AppState>) {
    state.runner.stop_all();
}

#[tauri::command]
pub fn get_output(state: State<'_, AppState>, id: String) -> Vec<Chunk> {
    state.runner.output(&id)
}

#[tauri::command]
pub fn clear_output(state: State<'_, AppState>, id: String) {
    state.runner.clear_output(&id);
}

/// Нажатия клавиш из окна терминала. Запись в канал консоли может ждать, поэтому не в главном потоке.
#[tauri::command(async)]
pub fn term_input(state: State<'_, AppState>, id: String, data: String) {
    state.runner.input(&id, &data);
}

#[tauri::command(async)]
pub fn term_resize(state: State<'_, AppState>, cols: u16, rows: u16) {
    state.runner.resize(cols, rows);
}

/// Строка вкладки портов: запомненный порт и то, открыт ли он прямо сейчас.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortRow {
    port: u16,
    open: bool,
    pid: Option<u32>,
    process: String,
    owner: Option<String>,
    first_seen: u64,
    last_seen: u64,
}

/// Память о портах пишется на диск сразу при новой записи, а сдвиг времени — не чаще этого.
const PORTS_SAVE_EVERY: Duration = Duration::from_secs(300);

/// Снимает таблицу слушающих портов, дополняет память и возвращает её целиком.
pub fn scan_ports(state: &AppState) -> Vec<PortRow> {
    let owners = state.runner.owners();
    let labels = state.labels();

    // Один порт часто слушают дважды, по IPv4 и по IPv6: оставляем одну запись,
    // предпочитая ту, у которой известен сервер менеджера.
    let mut open: BTreeMap<u16, Sighting> = BTreeMap::new();
    for (pid, port) in ports::listening() {
        let owner = owners.get(&pid).and_then(|id| labels.get(id)).cloned();
        if open.get(&port).is_some_and(|prev| prev.owner.is_some() || owner.is_none()) {
            continue;
        }
        open.insert(port, Sighting { port, pid, process: String::new(), owner });
    }
    let names = ports::process_names();
    for s in open.values_mut() {
        if worth_remembering(s.port, s.owner.is_some()) {
            s.process = names.get(&s.pid).cloned().unwrap_or_default();
        }
    }
    let sightings: Vec<Sighting> = open.values().cloned().collect();

    let mut guard = state.ports.lock().unwrap_or_else(|e| e.into_inner());
    let (book, saved) = &mut *guard;
    let changed = book.observe(now_ms(), &sightings);
    if changed || saved.elapsed() >= PORTS_SAVE_EVERY {
        let _ = book.save(&state.ports_file());
        *saved = Instant::now();
    }
    book.entries()
        .map(|e| {
            let live = open.get(&e.port);
            PortRow {
                port: e.port,
                open: live.is_some(),
                pid: live.map(|s| s.pid),
                process: e.process.clone(),
                owner: e.owner.clone(),
                first_seen: e.first_seen,
                last_seen: e.last_seen,
            }
        })
        .collect()
}

#[tauri::command(async)]
pub fn get_ports(state: State<'_, AppState>) -> Vec<PortRow> {
    scan_ports(&state)
}

/// Убирает порт из памяти. Открытый сейчас порт тут же запомнится снова, и это правильно.
#[tauri::command(async)]
pub fn forget_port(state: State<'_, AppState>, port: u16) -> Vec<PortRow> {
    {
        let mut guard = state.ports.lock().unwrap_or_else(|e| e.into_inner());
        if guard.0.forget(port) {
            let _ = guard.0.save(&state.ports_file());
        }
    }
    scan_ports(&state)
}

#[tauri::command(async)]
pub fn save_settings(app: AppHandle, state: State<'_, AppState>, mut settings: Settings) -> Result<Config, String> {
    settings.normalize();
    let autostart_changed = state.config().settings.launch_with_windows != settings.launch_with_windows;
    // Сначала система, потом файл: если запись в автозагрузку не удалась, галочка не должна
    // остаться включённой и обещать то, чего нет.
    if autostart_changed {
        let autolaunch = app.autolaunch();
        let result = if settings.launch_with_windows { autolaunch.enable() } else { autolaunch.disable() };
        result.map_err(|e| format!("Не удалось изменить автозапуск: {e}"))?;
    }

    let saved = {
        let mut cfg = state.config();
        cfg.settings = settings;
        state.save(&cfg)?;
        cfg.clone()
    };
    state.runner.set_scrollback(saved.settings.log_lines);
    Ok(saved)
}

#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let url = url.trim();
    // Сюда попадает адрес из вывода процесса: открываем только то, что точно веб-страница.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Открыть можно только адрес http или https".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !Path::new(&path).is_dir() {
        return Err(format!("Папка не найдена: {path}"));
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn quit_app(app: AppHandle) {
    crate::quit(&app);
}

/// Серверы с флажком «запускать при старте приложения».
pub fn start_marked(app: &AppHandle) {
    let state = app.state::<AppState>();
    start_where(&state, |autostart| autostart);
}

fn start_where(state: &AppState, pick: impl Fn(bool) -> bool) -> usize {
    let specs: Vec<_> = state
        .config()
        .projects
        .iter()
        .flat_map(|p| p.launchers.iter())
        .filter(|l| pick(l.autostart))
        .map(spec_of)
        .collect();
    specs.iter().filter(|spec| state.runner.start(spec).is_err()).count()
}

/// Запуск или остановка из меню трея.
pub fn toggle(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let status = state.runner.snapshot().into_iter().find(|v| v.id == id).map(|v| v.status).unwrap_or_default();
    if status == Status::Paused {
        let _ = state.runner.resume(id);
    } else if status.is_alive() {
        state.runner.stop(id);
    } else if let Some(spec) = state.spec(id) {
        // Об ошибке расскажет журнал сервера и статус «упал» в окне.
        let _ = state.runner.start(&spec);
    }
}

