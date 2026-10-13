//! Значок в трее: приложение живёт здесь, когда окно закрыто.

use crate::{commands, request_quit, show_main, AppState, APP_NAME, MAIN_WINDOW};
use lsm_core::{
    badge::{self, Bitmap},
    runner::Status,
};
use std::{collections::HashMap, sync::Mutex};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, Wry,
};

const TRAY_ID: &str = "main";
const TOGGLE_PREFIX: &str = "toggle:";

/// Отпечаток того, что сейчас показано в меню. Меню пересобирается только при его смене:
/// события статусов идут часто, а пересборка на каждое закрывала бы открытое меню под рукой.
#[derive(Default)]
pub struct TrayState(Mutex<String>);

struct Entry {
    id: String,
    label: String,
    status: Status,
}

fn entries(app: &AppHandle) -> Vec<Entry> {
    let state = app.state::<AppState>();
    let statuses: HashMap<String, Status> = state.runner.snapshot().into_iter().map(|v| (v.id, v.status)).collect();
    state
        .labels_in_order()
        .into_iter()
        .map(|(id, label)| {
            let status = statuses.get(&id).copied().unwrap_or_default();
            Entry { id, label, status }
        })
        .collect()
}

fn running(entries: &[Entry]) -> usize {
    entries.iter().filter(|e| e.status.is_running()).count()
}

fn tooltip(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return APP_NAME.to_string();
    }
    let paused = entries.iter().filter(|e| e.status == Status::Paused).count();
    let mut text = format!("{APP_NAME}: работает {} из {}", running(entries), entries.len());
    if paused > 0 {
        text.push_str(&format!(", на паузе {paused}"));
    }
    text
}

fn build_menu(app: &AppHandle, entries: &[Entry]) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    items.push(Box::new(MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?));

    if !entries.is_empty() {
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        for e in entries {
            // Галочка означает «запущен». Клик останавливает работающий, запускает остановленный
            // и снимает с паузы приостановленный.
            let id = format!("{TOGGLE_PREFIX}{}", e.id);
            let label = if e.status == Status::Paused { format!("{} (на паузе)", e.label) } else { e.label.clone() };
            items.push(Box::new(CheckMenuItem::with_id(app, id, label, true, e.status.is_alive(), None::<&str>)?));
        }
        let any_alive = entries.iter().any(|e| e.status.is_alive());
        let any_idle = entries.iter().any(|e| !e.status.is_alive());
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
        items.push(Box::new(MenuItem::with_id(app, "start_all", "Запустить все", any_idle, None::<&str>)?));
        items.push(Box::new(MenuItem::with_id(app, "stop_all", "Остановить все", any_alive, None::<&str>)?));
    }

    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?));

    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|i| i.as_ref()).collect();
    Menu::with_items(app, &refs)
}

fn image(bitmap: Bitmap) -> Image<'static> {
    Image::new_owned(bitmap.rgba, bitmap.width, bitmap.height)
}

/// Показывает число работающих серверов: значком в трее и меткой на кнопке в панели задач.
/// Вызывать в главном потоке.
fn show_count(app: &AppHandle, count: usize) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // Пока что-то работает, значок в трее сам становится числом; иначе обычный значок.
        let icon = if count > 0 { Some(image(badge::tray(count))) } else { app.default_window_icon().cloned() };
        let _ = tray.set_icon(icon);
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.set_overlay_icon(if count > 0 { Some(image(badge::overlay(count))) } else { None });
    }
}

/// Возвращает метку на кнопку в панели задач. Кнопка исчезает вместе со скрытым окном,
/// и после показа окна метку нужно поставить заново.
pub fn restore_count(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let count = running(&entries(&handle));
        show_count(&handle, count);
    });
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let list = entries(app);
    let menu = build_menu(app, &list)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(tooltip(&list))
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let id = event.id.as_ref().to_string();
    let app = app.clone();
    // Обработчик вызывается в главном потоке: запуск процесса и выход уводим из него.
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        match id.as_str() {
            "open" => show_main(&app),
            "start_all" => {
                commands::start_all(state);
            }
            "stop_all" => state.runner.stop_all(),
            "quit" => request_quit(&app),
            other => {
                if let Some(launcher) = other.strip_prefix(TOGGLE_PREFIX) {
                    commands::toggle(&app, launcher);
                    // Система сама переставила галочку по клику. Если запуск не удался,
                    // статус не изменится и обычное обновление меню не сработает: сверяем явно.
                    force_refresh(&app);
                }
            }
        }
    });
}

/// Приводит меню, подсказку и число на значке в соответствие с текущими статусами.
pub fn refresh(app: &AppHandle) {
    let list = entries(app);
    let print: String = list.iter().map(|e| format!("{}\u{1}{}\u{1}{:?}\u{2}", e.id, e.label, e.status)).collect();
    {
        let state = app.state::<TrayState>();
        let mut last = state.0.lock().unwrap_or_else(|e| e.into_inner());
        if *last == print {
            return;
        }
        *last = print;
    }

    let handle = app.clone();
    // Меню трея — объект главного потока, а статусы приходят из потоков процессов.
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id(TRAY_ID) {
            if let Ok(menu) = build_menu(&handle, &list) {
                let _ = tray.set_menu(Some(menu));
            }
            let _ = tray.set_tooltip(Some(tooltip(&list)));
        }
        show_count(&handle, running(&list));
    });
}

fn force_refresh(app: &AppHandle) {
    app.state::<TrayState>().0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    refresh(app);
}
