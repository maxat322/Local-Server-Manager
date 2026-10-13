//! Память о портах: какие номера уже были заняты на этой машине и кем.
//!
//! Нужна, чтобы новому проекту можно было выбрать порт, который не пересечётся ни с тем,
//! что работает сейчас, ни с тем, что обычно работает, но в данный момент выключено.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

/// С этого номера начинаются временные порты Windows: система раздаёт их службам и исходящим
/// соединениям как придётся, запоминать их бессмысленно. Проекту такой порт не назначают.
const DYNAMIC_FROM: u16 = 49152;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PortEntry {
    pub port: u16,
    /// Имя программы, которая слушала порт: `node.exe`, `postgres.exe`.
    pub process: String,
    /// Сервер менеджера, если порт открывал он: «SZ Learn» или «Mega Agent · web».
    pub owner: Option<String>,
    pub first_seen: u64,
    pub last_seen: u64,
}

/// Порт, который слушают прямо сейчас.
#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    pub port: u16,
    pub pid: u32,
    pub process: String,
    pub owner: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(default)]
pub struct PortBook {
    entries: BTreeMap<u16, PortEntry>,
}

/// Стоит ли помнить порт. Временные запоминаются, только если их открыл сервер менеджера:
/// тогда это осознанный выбор проекта, а не случайность.
pub fn worth_remembering(port: u16, owned: bool) -> bool {
    owned || port < DYNAMIC_FROM
}

impl PortBook {
    pub fn load(path: &Path) -> PortBook {
        fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// Учитывает снимок слушающих портов. Возвращает `true`, если появилось что-то новое
    /// по существу (порт или его хозяин), а не просто обновилось время.
    pub fn observe(&mut self, now: u64, seen: &[Sighting]) -> bool {
        let mut changed = false;
        for s in seen {
            if !worth_remembering(s.port, s.owner.is_some()) {
                continue;
            }
            match self.entries.get_mut(&s.port) {
                Some(entry) => {
                    entry.last_seen = now;
                    // Порт занял кто-то другой: прежняя подпись про него больше не говорит.
                    let other_program = !s.process.is_empty() && s.process != entry.process;
                    if other_program {
                        entry.process = s.process.clone();
                        entry.owner = s.owner.clone();
                        changed = true;
                    } else if s.owner.is_some() && s.owner != entry.owner {
                        // Та же программа, но без хозяина — сервер запустили мимо менеджера:
                        // подпись проекта при этом остаётся верной, её не стираем.
                        entry.owner = s.owner.clone();
                        changed = true;
                    }
                }
                None => {
                    self.entries.insert(
                        s.port,
                        PortEntry {
                            port: s.port,
                            process: s.process.clone(),
                            owner: s.owner.clone(),
                            first_seen: now,
                            last_seen: now,
                        },
                    );
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn forget(&mut self, port: u16) -> bool {
        self.entries.remove(&port).is_some()
    }

    pub fn entries(&self) -> impl Iterator<Item = &PortEntry> {
        self.entries.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::new_id;

    fn seen(port: u16, process: &str, owner: Option<&str>) -> Sighting {
        Sighting { port, pid: 1, process: process.into(), owner: owner.map(String::from) }
    }

    fn entry(book: &PortBook, port: u16) -> PortEntry {
        book.entries().find(|e| e.port == port).cloned().unwrap()
    }

    #[test]
    fn new_port_is_remembered_and_time_moves() {
        let mut book = PortBook::default();
        assert!(book.observe(100, &[seen(5173, "node.exe", Some("SZ Learn"))]));
        // Тот же порт позже: по существу ничего нового, но время последней встречи сдвинулось.
        assert!(!book.observe(900, &[seen(5173, "node.exe", Some("SZ Learn"))]));
        let e = entry(&book, 5173);
        assert_eq!((e.first_seen, e.last_seen), (100, 900));
        assert_eq!(e.owner.as_deref(), Some("SZ Learn"));
    }

    #[test]
    fn closed_port_stays_in_the_book() {
        let mut book = PortBook::default();
        book.observe(100, &[seen(7430, "node.exe", None), seen(5432, "postgres.exe", None)]);
        book.observe(200, &[seen(5432, "postgres.exe", None)]);
        assert_eq!(book.entries().map(|e| e.port).collect::<Vec<_>>(), [5432, 7430]);
        assert_eq!(entry(&book, 7430).last_seen, 100);
    }

    #[test]
    fn project_label_survives_a_run_outside_the_manager() {
        let mut book = PortBook::default();
        book.observe(100, &[seen(7430, "node.exe", Some("SZ Learn"))]);
        assert!(!book.observe(200, &[seen(7430, "node.exe", None)]));
        assert_eq!(entry(&book, 7430).owner.as_deref(), Some("SZ Learn"));
    }

    #[test]
    fn another_program_on_the_port_replaces_the_label() {
        let mut book = PortBook::default();
        book.observe(100, &[seen(8000, "node.exe", Some("Boom cats"))]);
        assert!(book.observe(200, &[seen(8000, "python.exe", None)]));
        let e = entry(&book, 8000);
        assert_eq!((e.process.as_str(), e.owner), ("python.exe", None));
        assert_eq!(e.first_seen, 100);
    }

    #[test]
    fn dynamic_ports_are_kept_only_for_own_servers() {
        let mut book = PortBook::default();
        book.observe(100, &[seen(49670, "svchost.exe", None), seen(51234, "node.exe", Some("Тест"))]);
        assert_eq!(book.entries().map(|e| e.port).collect::<Vec<_>>(), [51234]);
    }

    #[test]
    fn forget_and_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lsm-ports-{}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ports.json");

        let mut book = PortBook::default();
        book.observe(100, &[seen(3000, "node.exe", Some("API")), seen(5175, "node.exe", None)]);
        assert!(book.forget(5175));
        assert!(!book.forget(5175));
        book.save(&file).unwrap();

        let back = PortBook::load(&file);
        assert_eq!(back.entries().cloned().collect::<Vec<_>>(), book.entries().cloned().collect::<Vec<_>>());
        // Битый или отсутствующий файл — просто пустая книга.
        fs::write(&file, "{ oops").unwrap();
        assert_eq!(PortBook::load(&file).entries().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
