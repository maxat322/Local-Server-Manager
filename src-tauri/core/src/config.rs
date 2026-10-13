//! Настройки и список репозиториев. Всё лежит в одном `config.json`: записей здесь десяток,
//! база данных ради них не нужна.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const KIND_NPM: &str = "npm";
pub const KIND_SCRIPT: &str = "script";
pub const KIND_COMMAND: &str = "command";

const LOG_LINES_MIN: usize = 200;
const LOG_LINES_MAX: usize = 50_000;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub accent: String,
    pub launch_with_windows: bool,
    pub start_hidden: bool,
    pub close_to_tray: bool,
    pub notify_on_crash: bool,
    pub log_lines: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            accent: "mint".into(),
            launch_with_windows: false,
            start_hidden: true,
            close_to_tray: true,
            notify_on_crash: true,
            log_lines: 5000,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Launcher {
    pub id: String,
    pub title: String,
    /// `npm`, `script` или `command`: от этого зависит, чем запускать `command`.
    pub kind: String,
    /// Готовая строка: `pnpm run dev`, полный путь к файлу или произвольная команда.
    pub command: String,
    pub cwd: String,
    /// Адрес, заданный вручную. Пусто — берётся найденный в выводе или по слушающему порту.
    pub url: Option<String>,
    pub autostart: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub launchers: Vec<Launcher>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub settings: Settings,
    pub projects: Vec<Project>,
}

impl Settings {
    pub fn normalize(&mut self) {
        self.log_lines = self.log_lines.clamp(LOG_LINES_MIN, LOG_LINES_MAX);
        if self.accent.trim().is_empty() {
            self.accent = "mint".into();
        }
    }
}

impl Project {
    /// Приводит запись из интерфейса к годному виду: выдаёт идентификаторы, чистит пробелы,
    /// выбрасывает способы запуска без команды.
    pub fn normalize(&mut self) -> Result<(), String> {
        self.path = self.path.trim().to_string();
        self.name = self.name.trim().to_string();
        if self.path.is_empty() {
            return Err("Не указана папка репозитория".into());
        }
        if self.name.is_empty() {
            self.name = folder_name(&self.path);
        }
        if self.id.is_empty() {
            self.id = new_id();
        }

        self.launchers.retain(|l| !l.command.trim().is_empty());
        if self.launchers.is_empty() {
            return Err("Нужен хотя бы один способ запуска".into());
        }
        for l in &mut self.launchers {
            l.command = l.command.trim().to_string();
            l.cwd = l.cwd.trim().to_string();
            if l.cwd.is_empty() {
                l.cwd = self.path.clone();
            }
            l.title = l.title.trim().to_string();
            if l.title.is_empty() {
                l.title = l.command.clone();
            }
            if !matches!(l.kind.as_str(), KIND_NPM | KIND_SCRIPT | KIND_COMMAND) {
                l.kind = KIND_COMMAND.into();
            }
            l.url = l.url.take().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
            if l.id.is_empty() {
                l.id = new_id();
            }
        }
        Ok(())
    }
}

pub fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Один и тот же путь, записанный по-разному: Windows не различает регистр и слэши.
pub fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

/// Идентификатор из времени и счётчика: уникален в пределах одного конфига, а большего не нужно.
pub fn new_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:03x}", nanos & 0xffff_ffff_ffff, n & 0xfff)
}

/// Нет файла — это первый запуск, а не ошибка. Битый файл не затираем: откладываем рядом
/// как `config.json.bad`, иначе первая же запись уничтожит то, что можно было починить руками.
pub fn load(path: &Path) -> (Config, Option<String>) {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return (Config::default(), None),
    };
    match serde_json::from_str::<Config>(&text) {
        Ok(mut cfg) => {
            cfg.settings.normalize();
            (cfg, None)
        }
        Err(e) => {
            let bad = path.with_extension("json.bad");
            let _ = fs::copy(path, &bad);
            (
                Config::default(),
                Some(format!("Файл настроек не прочитался ({e}). Копия сохранена: {}", bad.display())),
            )
        }
    }
}

/// Пишем во временный файл и переименовываем: отключение питания посреди записи не оставит
/// половину конфига.
pub fn save(path: &Path, cfg: &Config) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать папку настроек: {e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("Не удалось записать настройки: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("Не удалось сохранить настройки: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_fills_ids_and_defaults() {
        let mut p = Project {
            path: " C:\\Dev\\SZ Learn ".into(),
            launchers: vec![
                Launcher { command: " npm run dev ".into(), kind: KIND_NPM.into(), ..Default::default() },
                Launcher { command: "   ".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        p.normalize().unwrap();
        assert_eq!(p.name, "SZ Learn");
        assert_eq!(p.launchers.len(), 1);
        let l = &p.launchers[0];
        assert_eq!(l.command, "npm run dev");
        assert_eq!(l.title, "npm run dev");
        assert_eq!(l.cwd, "C:\\Dev\\SZ Learn");
        assert!(!p.id.is_empty() && !l.id.is_empty() && p.id != l.id);
    }

    #[test]
    fn paths_compare_without_case_and_trailing_slash() {
        assert!(same_path("C:\\Dev\\SZ Learn", "c:/dev/sz learn/"));
        assert!(!same_path("C:\\Dev\\SZapp", "C:\\Dev\\SZapp-deal-pipelines"));
    }

    #[test]
    fn normalize_rejects_project_without_launchers() {
        let mut p = Project { path: "C:\\x".into(), ..Default::default() };
        assert!(p.normalize().is_err());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lsm-cfg-{}", new_id()));
        let file = dir.join("config.json");
        let mut cfg = Config::default();
        cfg.settings.accent = "violet".into();
        cfg.projects.push(Project { id: "p".into(), name: "Проект".into(), path: "C:\\p".into(), launchers: vec![] });
        save(&file, &cfg).unwrap();
        let (back, err) = load(&file);
        assert!(err.is_none());
        assert_eq!(back, cfg);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_is_kept_aside() {
        let dir = std::env::temp_dir().join(format!("lsm-cfg-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        fs::write(&file, "{ не json").unwrap();
        let (cfg, err) = load(&file);
        assert_eq!(cfg, Config::default());
        assert!(err.is_some());
        assert!(dir.join("config.json.bad").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"settings":{"accent":"blue"}}"#).unwrap();
        assert_eq!(cfg.settings.accent, "blue");
        assert!(cfg.settings.close_to_tray);
        assert_eq!(cfg.settings.log_lines, 5000);
    }
}
