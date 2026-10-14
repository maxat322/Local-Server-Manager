//! Серверы, запущенные мимо менеджера: из своего терминала, двойным щелчком по .bat, другой копией.
//!
//! Такой запуск держит порт, и запуск из менеджера после него сразу выходит: «порт занят».
//! Чужим считается процесс, который слушает TCP-порт и в чьём пути к программе, командной строке
//! или текущей папке есть папка репозитория. Текущая папка нужна для запусков с относительным
//! путём (`node panel/server.js`). Порт обязателен: из папки проекта работают и редакторы,
//! и языковые серверы, но порты они не слушают.

use crate::ports;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::c_void,
    mem, ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        },
        Threading::{
            OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_TERMINATE,
        },
    },
};

// Командную строку чужого процесса документированный API не отдаёт. Этот класс запроса есть
// с Windows 8.1 и требует того же права, что и путь к программе.
#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(process: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn ReadProcessMemory(process: HANDLE, at: *const c_void, buf: *mut c_void, len: usize, read: *mut usize) -> i32;
}
const PROCESS_BASIC_INFORMATION: u32 = 0;
const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const PROCESS_VM_READ: u32 = 0x0010;
/// Смещения в блоке окружения 64-битного процесса: параметры запуска в PEB и текущая папка в них.
/// Они не менялись со времён Windows Vista; ими же пользуются Process Explorer и Process Hacker.
const PEB_PROCESS_PARAMETERS: usize = 0x20;
const PARAMETERS_CURRENT_DIRECTORY: usize = 0x38;

#[repr(C)]
#[derive(Default)]
struct BasicInformation {
    exit_status: isize,
    peb: usize,
    affinity: usize,
    base_priority: isize,
    pid: usize,
    parent: usize,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

/// Программы, которые открывают папку проекта и могут слушать порт, но сервером не являются.
const NOT_SERVERS: &[&str] = &[
    "code.exe", "cursor.exe", "windsurf.exe", "devenv.exe", "idea64.exe", "webstorm64.exe", "pycharm64.exe",
    "rider64.exe", "explorer.exe", "windowsterminal.exe", "openconsole.exe", "conhost.exe", "claude.exe",
    "msedge.exe", "chrome.exe", "firefox.exe", "msedgewebview2.exe",
];

/// Что искать: способ запуска и его папки, от самой точной к общей (папка запуска, репозиторий).
#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub paths: Vec<String>,
}

/// Найденный чужой запуск.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// Способ запуска, которому он принадлежит.
    pub id: String,
    /// Верхний процесс запуска: остановка гасит его вместе со всеми потомками.
    pub root: u32,
    pub pids: Vec<u32>,
    /// Программа, которая слушает порт: `node.exe`, `python.exe`.
    pub name: String,
    pub ports: Vec<u16>,
    /// Командная строка верхнего процесса: по ней видно, откуда запуск.
    pub command: String,
}

struct Proc {
    parent: u32,
    name: String,
}

/// Чужие запуски способов из `targets`. Процессы из `exclude` (серверы самого менеджера)
/// и дерево самого приложения не рассматриваются.
pub fn detect(targets: &[Target], exclude: &HashSet<u32>) -> Vec<Found> {
    if targets.is_empty() {
        return Vec::new();
    }
    let procs = snapshot();
    let mut skip = exclude.clone();
    skip.extend(descendants(&procs, std::process::id()));

    let ids: Vec<&str> = targets.iter().map(|t| t.id.as_str()).collect();
    let targets: Vec<(usize, Vec<String>)> =
        targets.iter().enumerate().map(|(i, t)| (i, t.paths.iter().map(|p| normalize(p)).collect())).collect();
    let mut texts: HashMap<u32, String> = HashMap::new();
    let mut text = |pid: u32| texts.entry(pid).or_insert_with(|| describe(pid)).clone();
    let usable = |pid: u32| {
        pid > 4 && !skip.contains(&pid) && procs.get(&pid).is_some_and(|p| !NOT_SERVERS.contains(&p.name.to_lowercase().as_str()))
    };

    // Верхний процесс → (способ запуска, слушающий процесс, порты).
    let mut roots: BTreeMap<u32, (usize, u32, Vec<u16>)> = BTreeMap::new();
    for (pid, port) in ports::listening() {
        if !usable(pid) {
            continue;
        }
        // У самого сервера в счёт идёт и текущая папка. У родителей — нет: терминал пользователя,
        // открытый в папке проекта, сервером не считается, и его остановка закрыла бы вкладку.
        let own = format!("{} {}", text(pid), current_dir(pid).map(|d| normalize(&d)).unwrap_or_default());
        let Some((target, path)) = best_target(&own, &targets) else { continue };
        // Поднимаемся к родителям, пока в их командной строке та же папка: `cmd /c start-local.cmd`
        // над `node`, `concurrently` над несколькими `node`.
        let mut root = pid;
        let mut seen = HashSet::from([pid]);
        while let Some(parent) = procs.get(&root).map(|p| p.parent) {
            if !seen.insert(parent) || !usable(parent) || !contains_path(&text(parent), &path) {
                break;
            }
            root = parent;
        }
        let entry = roots.entry(root).or_insert((target, pid, Vec::new()));
        entry.2.push(port);
    }

    let mut found: Vec<Found> = roots
        .into_iter()
        .map(|(root, (target, listener, mut found_ports))| {
            found_ports.sort_unstable();
            found_ports.dedup();
            let mut pids: Vec<u32> = descendants(&procs, root).into_iter().filter(|p| !skip.contains(p)).collect();
            pids.sort_unstable();
            Found {
                id: ids[target].to_string(),
                root,
                pids,
                name: procs.get(&listener).map(|p| p.name.clone()).unwrap_or_default(),
                ports: found_ports,
                command: command_line_of(root).unwrap_or_default(),
            }
        })
        .collect();

    // Подъём к родителям обрывается на промежуточных `cmd /c …` без пути, и один запуск
    // (`local-services.mjs`, поднявший api и web) распадается на несколько. Запуск, лежащий
    // внутри дерева другого запуска того же сервера, вливается в него.
    let mut i = 0;
    while i < found.len() {
        let outer = (0..found.len())
            .find(|&j| j != i && found[j].id == found[i].id && found[j].pids.contains(&found[i].root));
        match outer {
            Some(j) => {
                let inner = found.remove(i);
                let j = if j > i { j - 1 } else { j };
                found[j].ports.extend(inner.ports);
                found[j].ports.sort_unstable();
                found[j].ports.dedup();
                i = 0;
            }
            None => i += 1,
        }
    }
    found
}

/// Гасит запуск: верхний процесс и всех его потомков. Возвращает, сколько процессов завершено.
pub fn kill_tree(root: u32) -> Result<usize, String> {
    let procs = snapshot();
    let me = std::process::id();
    let mine: HashSet<u32> = descendants(&procs, me).into_iter().collect();
    let mut killed = 0;
    let mut root_denied = false;
    // Сначала потомки: родитель вроде `concurrently` иначе успел бы заметить их смерть и поднять заново.
    for pid in descendants(&procs, root).into_iter().rev() {
        if pid <= 4 || mine.contains(&pid) {
            continue;
        }
        // SAFETY: дескриптор закрывается здесь же.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                root_denied |= pid == root;
                continue;
            }
            if TerminateProcess(handle, 1) != 0 {
                killed += 1;
            }
            CloseHandle(handle);
        }
    }
    if root_denied && killed == 0 {
        return Err("Нет прав остановить этот процесс: похоже, он запущен от имени администратора".into());
    }
    Ok(killed)
}

/// Самая точная цель, чья папка встречается в описании процесса, и сама эта папка.
fn best_target(text: &str, targets: &[(usize, Vec<String>)]) -> Option<(usize, String)> {
    let mut best: Option<(usize, String)> = None;
    for (index, paths) in targets {
        for path in paths {
            if !path.is_empty()
                && contains_path(text, path)
                && best.as_ref().is_none_or(|(_, b)| path.len() > b.len())
            {
                best = Some((*index, path.clone()));
            }
        }
    }
    best
}

/// Папка входит в текст целиком: `c:\dev\app` не должна находиться в `c:\dev\app-old\server.js`.
fn contains_path(text: &str, path: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(path) {
        let end = from + at + path.len();
        match text[end..].chars().next() {
            None | Some('\\' | '"' | '\'' | ' ') => return true,
            _ => from = from + at + 1,
        }
    }
    false
}

/// Путь для сравнения: строчные буквы, обратные косые черты, без косой черты на конце.
fn normalize(path: &str) -> String {
    path.trim().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Путь к программе и командная строка процесса одной строкой, уже нормализованной.
fn describe(pid: u32) -> String {
    let image = image_path(pid).unwrap_or_default();
    let line = command_line_of(pid).unwrap_or_default();
    normalize(&format!("{image} {line}"))
}

fn image_path(pid: u32) -> Option<String> {
    // SAFETY: дескриптор закрывается здесь же; размер буфера передан в символах.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        (ok != 0).then(|| String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// Текущая папка процесса. Читается из его памяти, поэтому нужен процесс той же учётной записи.
fn current_dir(pid: u32) -> Option<String> {
    // SAFETY: читаем чужую память только через ReadProcessMemory с проверкой прочитанного;
    // дескриптор закрывается здесь же.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }
        let read = |at: usize, buf: *mut c_void, len: usize| {
            let mut got = 0usize;
            ReadProcessMemory(handle, at as *const c_void, buf, len, &mut got) != 0 && got == len
        };
        let mut info = BasicInformation::default();
        let mut result = None;
        let ok = NtQueryInformationProcess(
            handle,
            PROCESS_BASIC_INFORMATION,
            &mut info as *mut _ as *mut c_void,
            mem::size_of::<BasicInformation>() as u32,
            ptr::null_mut(),
        ) >= 0;
        let mut params = 0usize;
        if ok && info.peb != 0 && read(info.peb + PEB_PROCESS_PARAMETERS, &mut params as *mut _ as *mut c_void, 8) {
            let mut dir = UnicodeString { length: 0, maximum: 0, buffer: ptr::null() };
            let at = params + PARAMETERS_CURRENT_DIRECTORY;
            if read(at, &mut dir as *mut _ as *mut c_void, mem::size_of::<UnicodeString>())
                && dir.length > 0
                && dir.length <= 2048
            {
                let mut wide = vec![0u16; dir.length as usize / 2];
                if read(dir.buffer as usize, wide.as_mut_ptr() as *mut c_void, dir.length as usize) {
                    result = Some(String::from_utf16_lossy(&wide));
                }
            }
        }
        CloseHandle(handle);
        result
    }
}

fn command_line_of(pid: u32) -> Option<String> {
    // SAFETY: первый вызов только сообщает размер; второй пишет в буфер не больше заявленного.
    // Строка лежит в том же буфере, сразу за заголовком.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut need: u32 = 0;
        NtQueryInformationProcess(handle, PROCESS_COMMAND_LINE_INFORMATION, ptr::null_mut(), 0, &mut need);
        let mut result = None;
        if need >= mem::size_of::<UnicodeString>() as u32 {
            let mut buf = vec![0u64; need as usize / 8 + 1];
            let status = NtQueryInformationProcess(
                handle,
                PROCESS_COMMAND_LINE_INFORMATION,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * 8) as u32,
                &mut need,
            );
            let s = &*(buf.as_ptr() as *const UnicodeString);
            if status >= 0 && !s.buffer.is_null() {
                result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(s.buffer, s.length as usize / 2)));
            }
        }
        CloseHandle(handle);
        result
    }
}

/// Все процессы системы: номер → родитель и имя программы.
fn snapshot() -> HashMap<u32, Proc> {
    let mut procs = HashMap::new();
    // SAFETY: снимок закрывается здесь же, размер структуры задан перед первым вызовом.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return procs;
        }
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry);
        while more != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            procs.insert(
                entry.th32ProcessID,
                Proc { parent: entry.th32ParentProcessID, name: String::from_utf16_lossy(&entry.szExeFile[..len]) },
            );
            more = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    procs
}

/// Процесс и все его потомки, родители раньше детей.
fn descendants(procs: &HashMap<u32, Proc>, root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, p) in procs {
        if pid != p.parent {
            children.entry(p.parent).or_default().push(pid);
        }
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = children.get(&out[i]) {
            for &kid in kids {
                if !out.contains(&kid) {
                    out.push(kid);
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::new_id;
    use std::{
        fs,
        net::TcpStream,
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    fn targets(list: &[(&str, &[&str])]) -> Vec<(usize, Vec<String>)> {
        list.iter().enumerate().map(|(i, (_, paths))| (i, paths.iter().map(|p| normalize(p)).collect())).collect()
    }

    #[test]
    fn path_must_match_as_a_whole_folder() {
        let text = normalize(r#""C:\Program Files\nodejs\node.exe" C:\Dev\App\node_modules\vite\bin\vite.js"#);
        assert!(contains_path(&text, &normalize(r"C:\Dev\App")));
        assert!(contains_path(&text, &normalize("c:/dev/app/")));
        assert!(!contains_path(&text, &normalize(r"C:\Dev\Ap")));
        assert!(!contains_path(&normalize(r"node C:\Dev\App-old\server.js"), &normalize(r"C:\Dev\App")));
        assert!(contains_path(&normalize(r#"cmd /c "C:\Dev\App""#), &normalize(r"C:\Dev\App")));
        assert!(contains_path(&normalize(r"C:\Dev\App"), &normalize(r"C:\Dev\App")));
    }

    #[test]
    fn most_specific_folder_wins() {
        let list = targets(&[("root", &[r"C:\Dev\Mono"]), ("web", &[r"C:\Dev\Mono\web", r"C:\Dev\Mono"])]);
        let web = normalize(r"node C:\Dev\Mono\web\node_modules\vite\bin\vite.js");
        assert_eq!(best_target(&web, &list).map(|(i, _)| i), Some(1));
        let api = normalize(r"node C:\Dev\Mono\api\server.js");
        assert_eq!(best_target(&api, &list).map(|(i, _)| i), Some(0));
        assert_eq!(best_target(&normalize(r"node C:\Other\x.js"), &list), None);
    }

    #[test]
    fn descendants_come_after_parents() {
        let procs: HashMap<u32, Proc> = [(10, 1), (11, 10), (12, 11), (13, 10), (20, 1)]
            .into_iter()
            .map(|(pid, parent)| (pid, Proc { parent, name: String::new() }))
            .collect();
        let tree = descendants(&procs, 10);
        assert_eq!(tree[0], 10);
        assert_eq!(tree.len(), 4);
        assert!(tree.iter().position(|&p| p == 11) < tree.iter().position(|&p| p == 12));
        assert!(!tree.contains(&20));
    }

    /// Сервер, поднятый мимо менеджера через `cmd /c node …`, находится вместе с родителем
    /// и гасится целиком; процессы менеджера в поиск не попадают.
    #[test]
    fn server_started_elsewhere_is_found_and_stopped() {
        let dir = std::env::temp_dir().join(format!("lsm foreign {}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.js");
        fs::write(&script, "require('http').createServer((q, s) => s.end('ok')).listen(0, '127.0.0.1')").unwrap();
        // Сервер не должен быть потомком теста: потомков самого приложения поиск пропускает.
        // Промежуточный cmd запускает его через `start /b` и сразу выходит. Вывод в никуда,
        // чтобы осиротевшие процессы не держали вывод теста при неудаче.
        let mut launcher = Command::new("cmd")
            .raw_arg(format!(r#"/d /c start "" /b cmd /d /c node "{}""#, script.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        launcher.wait().unwrap();

        let target = Target { id: "srv".into(), paths: vec![dir.to_string_lossy().into_owned()] };
        let deadline = Instant::now() + Duration::from_secs(15);
        let found = loop {
            let found = detect(std::slice::from_ref(&target), &HashSet::new());
            if !found.is_empty() {
                break found;
            }
            assert!(Instant::now() < deadline, "чужой сервер не нашёлся");
            thread::sleep(Duration::from_millis(200));
        };
        assert_eq!(found.len(), 1, "{found:?}");
        let f = &found[0];
        assert_eq!(f.id, "srv");
        assert_eq!(f.name.to_lowercase(), "node.exe");
        assert_eq!(f.pids.len(), 2, "верхним должен быть cmd, запустивший node: {f:?}");
        assert!(f.command.to_lowercase().starts_with("cmd"), "{f:?}");
        assert!(f.command.to_lowercase().contains("server.js"));
        let port = f.ports[0];
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

        // Процессы менеджера исключаются: тот же сервер в его задании чужим не считается.
        let excluded: HashSet<u32> = f.pids.iter().copied().collect();
        assert!(detect(std::slice::from_ref(&target), &excluded).is_empty());

        assert_eq!(kill_tree(f.root), Ok(2));
        let deadline = Instant::now() + Duration::from_secs(5);
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(Instant::now() < deadline, "порт не освободился");
            thread::sleep(Duration::from_millis(100));
        }
        assert!(detect(&[target], &HashSet::new()).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Запуск с относительным путём (`node server.js` из папки проекта) находится по текущей
    /// папке процесса. Родитель с той же текущей папкой, но без пути в командной строке,
    /// в запуск не входит: так выглядит терминал пользователя, и его не трогаем.
    #[test]
    fn relative_start_is_found_by_working_folder() {
        let dir = std::env::temp_dir().join(format!("lsm foreign rel {}", new_id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("server.js"), "require('http').createServer((q, s) => s.end('ok')).listen(0, '127.0.0.1')")
            .unwrap();
        Command::new("cmd")
            .raw_arg(r#"/d /c start "" /b cmd /d /c node server.js"#)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();

        let target = Target { id: "rel".into(), paths: vec![dir.to_string_lossy().into_owned()] };
        let deadline = Instant::now() + Duration::from_secs(15);
        let found = loop {
            let found = detect(std::slice::from_ref(&target), &HashSet::new());
            if !found.is_empty() {
                break found;
            }
            assert!(Instant::now() < deadline, "сервер с относительным путём не нашёлся");
            thread::sleep(Duration::from_millis(200));
        };
        let f = &found[0];
        assert_eq!(f.pids, vec![f.root], "родитель без пути в команде в запуск не входит: {f:?}");
        assert!(f.command.contains("server.js"), "{f:?}");
        assert_eq!(kill_tree(f.root), Ok(1));
        let _ = fs::remove_dir_all(&dir);
        // Осиротевший cmd без node завершится сам: ему больше нечего ждать.
    }

    /// Что поиск нашёл бы на этой машине по настоящему списку репозиториев (только чтение):
    /// `LSM_SURVEY_CONFIG=<config.json> LSM_SURVEY_SKIP=<pid приложения> cargo test -p lsm-core survey -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn survey() {
        let path = std::env::var("LSM_SURVEY_CONFIG").expect("LSM_SURVEY_CONFIG");
        let (cfg, _) = crate::config::load(std::path::Path::new(&path));
        let targets: Vec<Target> = cfg
            .projects
            .iter()
            .flat_map(|p| p.launchers.iter().map(move |l| Target { id: format!("{} / {}", p.name, l.title), paths: vec![l.cwd.clone(), p.path.clone()] }))
            .collect();
        // Серверы работающего приложения — его потомки: их пропускаем, как пропустило бы оно само.
        let mut skip = HashSet::new();
        if let Some(app) = std::env::var("LSM_SURVEY_SKIP").ok().and_then(|v| v.parse().ok()) {
            skip.extend(descendants(&snapshot(), app));
        }
        for f in detect(&targets, &skip) {
            println!("{} | {} PID {} ({} проц.) порты {:?} | {}", f.id, f.name, f.root, f.pids.len(), f.ports, f.command);
        }
        println!("целей: {}", targets.len());
    }
}
