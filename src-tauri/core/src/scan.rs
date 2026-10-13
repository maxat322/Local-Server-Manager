//! Поиск способов запуска в папке репозитория. Только читает файлы, ничего не выполняет.

use crate::config::{folder_name, KIND_COMMAND, KIND_NPM, KIND_SCRIPT};
use serde::Serialize;
use serde_json::Value;
use std::{fs, path::Path};

/// Оценка, с которой кандидат попадает в основной список, а не в «другие».
const RECOMMENDED_FROM: u32 = 50;

/// Папки, в которых чужие или собранные `package.json`: искать там способ запуска бессмысленно.
const SKIP_DIRS: [&str; 10] = [
    "node_modules", "dist", "build", "target", "out", "coverage", "vendor", "venv", "__pycache__", "tmp",
];
/// Папки-контейнеры монорепозитория: пакеты лежат на уровень глубже.
const WORKSPACE_DIRS: [&str; 3] = ["apps", "packages", "services"];

const SCRIPT_EXTENSIONS: [&str; 3] = ["cmd", "bat", "ps1"];
const GOOD_FILE_WORDS: [&str; 7] = ["start", "run", "dev", "local", "serve", "launch", "up"];
const BAD_FILE_WORDS: [&str; 11] = [
    "stop", "kill", "install", "setup", "build", "deploy", "clean", "test", "uninstall", "update", "publish",
];

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub kind: String,
    pub title: String,
    pub command: String,
    pub cwd: String,
    /// Откуда взято: `package.json`, `web/package.json` или `файл`.
    pub source: String,
    /// Тело npm-скрипта: по нему видно, что на самом деле запустится.
    pub detail: String,
    pub score: u32,
    pub recommended: bool,
    /// О чём стоит знать до выбора: например, что скрипт сам открывает отдельное окно консоли.
    pub warning: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub name: String,
    pub path: String,
    pub candidates: Vec<Candidate>,
}

pub fn scan(root: &Path) -> Result<ScanResult, String> {
    if !root.is_dir() {
        return Err(format!("Папка не найдена: {}", root.display()));
    }
    let mut out = Vec::new();

    let root_manager = scan_package(root, root, None, &mut out);
    for dir in subdirs(root) {
        let name = folder_name(&dir.to_string_lossy());
        scan_package(root, &dir, root_manager.as_deref(), &mut out);
        if WORKSPACE_DIRS.contains(&name.as_str()) {
            for inner in subdirs(&dir) {
                scan_package(root, &inner, root_manager.as_deref(), &mut out);
            }
        }
    }
    scan_scripts(root, &mut out);

    // Сортировка устойчивая: при равной оценке сохраняется порядок обхода, корень раньше подпапок.
    out.sort_by(|a, b| b.score.cmp(&a.score));
    let path = root.to_string_lossy().into_owned();
    Ok(ScanResult { name: folder_name(&path), path, candidates: out })
}

fn subdirs(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            let name = folder_name(&p.to_string_lossy());
            !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str())
        })
        .collect();
    dirs.sort();
    dirs
}

/// Добавляет скрипты из `dir/package.json` и возвращает менеджер пакетов этой папки.
fn scan_package(root: &Path, dir: &Path, inherited: Option<&str>, out: &mut Vec<Candidate>) -> Option<String> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let manager = package_manager(dir, &json, inherited);

    let is_root = dir == root;
    let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().replace('\\', "/");
    let source = if is_root { "package.json".to_string() } else { format!("{rel}/package.json") };

    if let Some(scripts) = json.get("scripts").and_then(Value::as_object) {
        for (name, body) in scripts {
            let Some(body) = body.as_str() else { continue };
            if is_lifecycle_hook(name, scripts) {
                continue;
            }
            let command = format!("{manager} run {name}");
            let score = script_score(name, is_root);
            out.push(Candidate {
                kind: KIND_NPM.into(),
                title: if is_root { command.clone() } else { format!("{rel} · {command}") },
                command,
                cwd: dir.to_string_lossy().into_owned(),
                source: source.clone(),
                detail: body.to_string(),
                score,
                recommended: score >= RECOMMENDED_FROM,
                warning: None,
            });
        }
    }
    Some(manager)
}

/// `predev` и `postbuild` запускаются сами вместе с основным скриптом: отдельным способом
/// запуска они не являются.
fn is_lifecycle_hook(name: &str, scripts: &serde_json::Map<String, Value>) -> bool {
    ["pre", "post"]
        .iter()
        .filter_map(|prefix| name.strip_prefix(prefix))
        .any(|rest| !rest.is_empty() && scripts.contains_key(rest))
}

/// Поле `packageManager` главнее lock-файлов: в репозитории после переезда на pnpm
/// старый `package-lock.json` нередко остаётся лежать.
fn package_manager(dir: &Path, json: &Value, inherited: Option<&str>) -> String {
    if let Some(field) = json.get("packageManager").and_then(Value::as_str) {
        let name = field.split('@').next().unwrap_or("").trim();
        if matches!(name, "npm" | "pnpm" | "yarn" | "bun") {
            return name.to_string();
        }
    }
    let has = |file: &str| dir.join(file).exists();
    if has("pnpm-lock.yaml") {
        "pnpm".into()
    } else if has("yarn.lock") {
        "yarn".into()
    } else if has("bun.lockb") || has("bun.lock") {
        "bun".into()
    } else if has("package-lock.json") {
        "npm".into()
    } else {
        // Пакет внутри монорепозитория своего lock-файла не имеет: менеджер тот же, что у корня.
        inherited.unwrap_or("npm").to_string()
    }
}

fn script_score(name: &str, is_root: bool) -> u32 {
    let base = match name {
        "dev" => 100,
        "start" => 80,
        "serve" | "develop" => 70,
        _ if ["dev:", "start:", "serve:"].iter().any(|p| name.starts_with(p)) => 50,
        "preview" | "watch" => 30,
        _ => 10,
    };
    // Скрипт подпапки запускает обычно одну часть проекта, а корневой — всё сразу.
    if is_root { base } else { base * 6 / 10 }
}

fn scan_scripts(root: &Path, out: &mut Vec<Candidate>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();

    for file in files {
        let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !SCRIPT_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let name = folder_name(&file.to_string_lossy());
        let stem = file.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        let score = file_score(&stem);

        // Пакетный файл может сам открыть отдельное окно консоли командой `start`. Тогда сервер
        // работает там, а во встроенный терминал попадают только строки самого файла.
        let windows = if ext == "ps1" { Vec::new() } else { own_windows(&file) };
        for inner in windows.iter().filter_map(|w| w.direct.as_ref()) {
            // Та же команда напрямую: без посредника её вывод идёт во встроенный терминал.
            // Оценка на единицу выше файла, чтобы в списке она стояла прямо над ним.
            out.push(Candidate {
                kind: KIND_COMMAND.into(),
                title: inner.clone(),
                command: inner.clone(),
                cwd: root.to_string_lossy().into_owned(),
                source: format!("из {name}"),
                detail: INNER_COMMAND_NOTE.into(),
                score: score + 1,
                recommended: score + 1 >= RECOMMENDED_FROM,
                warning: None,
            });
        }
        out.push(Candidate {
            kind: KIND_SCRIPT.into(),
            title: name,
            command: file.to_string_lossy().into_owned(),
            cwd: root.to_string_lossy().into_owned(),
            source: "файл".into(),
            detail: String::new(),
            score,
            recommended: score >= RECOMMENDED_FROM,
            warning: (!windows.is_empty()).then(|| OWN_WINDOW_WARNING.to_string()),
        });
    }
}

const OWN_WINDOW_WARNING: &str =
    "Сам открывает отдельное окно консоли (команда start): вывод сервера уйдёт туда, а не во встроенный терминал.";
const INNER_COMMAND_NOTE: &str = "Сервер из этого файла напрямую: без отдельного окна, вывод во встроенный терминал";

/// Программы, которые работают в консоли: `start` с ними открывает новое окно консоли.
/// Про остальные (оконные программы, документы) этого сказать нельзя, о них не предупреждаем.
const CONSOLE_PROGRAMS: [&str; 14] = [
    "cmd", "powershell", "pwsh", "python", "pythonw", "py", "node", "npm", "npx", "pnpm", "yarn", "bun", "deno",
    "uvicorn",
];
const CONSOLE_EXTENSIONS: [&str; 6] = ["bat", "cmd", "ps1", "py", "js", "mjs"];

/// Строка `start`, открывающая отдельное окно консоли.
#[derive(Debug, PartialEq)]
struct OwnWindow {
    /// Команда из этой строки, которую можно запускать напрямую. `None`, если это служебная
    /// мелочь вроде свёрнутого окна, ждущего сервер, чтобы открыть браузер.
    direct: Option<String>,
}

fn own_windows(file: &Path) -> Vec<OwnWindow> {
    let Ok(bytes) = fs::read(file) else { return Vec::new() };
    String::from_utf8_lossy(&bytes).lines().filter_map(parse_start).collect()
}

/// Разбирает строку пакетного файла. Возвращает `Some`, только если это `start`, который откроет
/// новое окно консоли: без ключа `/b` и с консольной программой, а не с адресом или документом.
fn parse_start(line: &str) -> Option<OwnWindow> {
    const SPACES: [char; 2] = [' ', '\t'];
    const QUOTE: char = '"';

    let line = line.trim().trim_start_matches('@').trim_start();
    let word = line.get(..5).filter(|w| w.eq_ignore_ascii_case("start"))?;
    let rest = &line[word.len()..];
    if !rest.starts_with(SPACES) {
        return None;
    }
    let mut rest = rest.trim_start();

    // Первый аргумент в кавычках — заголовок окна, а не программа.
    if rest.starts_with(QUOTE) {
        let end = rest[1..].find(QUOTE)? + 2;
        rest = rest[end..].trim_start();
    }
    // Ключи: /min, /wait, /b, /d <папка> и прочие.
    let mut minimized = false;
    while rest.starts_with('/') {
        let (flag, after) = split_token(rest);
        let flag = flag.to_ascii_lowercase();
        rest = after;
        match flag.as_str() {
            "/b" => return None,
            "/min" => minimized = true,
            "/d" => rest = split_token(rest).1,
            _ => {}
        }
    }
    if rest.is_empty() {
        // `start` без программы открывает пустое окно cmd.
        return Some(OwnWindow { direct: None });
    }

    let (program, args) = split_token(rest);
    let lower = program.trim_matches(QUOTE).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return None;
    }
    let file = lower.rsplit(['\\', '/']).next().unwrap_or("");
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    let name = if ext == "exe" { stem } else { file };
    if !CONSOLE_PROGRAMS.contains(&name) && !CONSOLE_EXTENSIONS.contains(&ext) {
        return None;
    }

    // `cmd /k "команда"` — обёртка: нужна сама команда.
    let inner = if name == "cmd" {
        let lower_args = args.to_ascii_lowercase();
        let at = lower_args.find("/k").or_else(|| lower_args.find("/c"));
        at.map(|i| unquote(args[i + 2..].trim())).unwrap_or_default()
    } else {
        rest.to_string()
    };
    // Свёрнутое окно и команды с адресами — это ожидание сервера и открытие браузера,
    // а не сам сервер: предлагать их способом запуска незачем.
    let helper = minimized || inner.is_empty() || inner.contains("http://") || inner.contains("https://");
    Some(OwnWindow { direct: (!helper).then_some(inner) })
}

/// Первый аргумент строки (в кавычках — целиком) и остаток.
fn split_token(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    let end = if text.starts_with('"') {
        text[1..].find('"').map_or(text.len(), |i| i + 2)
    } else {
        text.find([' ', '\t']).unwrap_or(text.len())
    };
    (&text[..end], text[end..].trim_start())
}

/// Снимает одну пару внешних кавычек: `"a b"` → `a b`.
fn unquote(text: &str) -> String {
    let inner = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
    inner.unwrap_or(text).to_string()
}

/// Сначала проверяются «плохие» слова: `setup` и `update` содержат `up`.
fn file_score(stem: &str) -> u32 {
    if BAD_FILE_WORDS.iter().any(|w| stem.contains(w)) {
        5
    } else if GOOD_FILE_WORDS.iter().any(|w| stem.contains(w)) {
        90
    } else {
        40
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::new_id;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("lsm-scan-{}", new_id()));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn file(&self, rel: &str, text: &str) -> &Self {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
            self
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn commands(result: &ScanResult) -> Vec<&str> {
        result.candidates.iter().map(|c| c.command.as_str()).collect()
    }

    #[test]
    fn npm_scripts_are_ranked() {
        let t = TempDir::new();
        t.file("package.json", r#"{"scripts":{"build":"vite build","start":"node s.js","dev":"vite","lint":"eslint ."}}"#)
            .file("package-lock.json", "{}");
        let r = scan(&t.0).unwrap();
        assert_eq!(commands(&r), ["npm run dev", "npm run start", "npm run build", "npm run lint"]);
        assert!(r.candidates[0].recommended && r.candidates[1].recommended);
        assert!(!r.candidates[2].recommended);
        assert_eq!(r.candidates[0].detail, "vite");
        assert_eq!(r.candidates[0].cwd, t.0.to_string_lossy());
    }

    #[test]
    fn package_manager_field_wins_over_lockfiles() {
        let t = TempDir::new();
        t.file("package.json", r#"{"packageManager":"pnpm@9.12.0","scripts":{"dev":"x"}}"#)
            .file("package-lock.json", "{}");
        assert_eq!(commands(&scan(&t.0).unwrap()), ["pnpm run dev"]);
    }

    #[test]
    fn pnpm_lock_wins_when_both_lockfiles_present() {
        let t = TempDir::new();
        t.file("package.json", r#"{"scripts":{"dev":"x"}}"#)
            .file("package-lock.json", "{}")
            .file("pnpm-lock.yaml", "");
        assert_eq!(commands(&scan(&t.0).unwrap()), ["pnpm run dev"]);
    }

    #[test]
    fn nested_packages_inherit_manager_and_keep_own_cwd() {
        let t = TempDir::new();
        t.file("package.json", r#"{"packageManager":"pnpm@11.0.0","scripts":{"dev":"pnpm -r dev"}}"#)
            .file("web/package.json", r#"{"scripts":{"dev":"vite"}}"#)
            .file("apps/api/package.json", r#"{"scripts":{"dev":"tsx watch"}}"#)
            .file("node_modules/dep/package.json", r#"{"scripts":{"dev":"nope"}}"#);
        let r = scan(&t.0).unwrap();
        assert_eq!(commands(&r), ["pnpm run dev", "pnpm run dev", "pnpm run dev"]);
        let titles: Vec<_> = r.candidates.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["pnpm run dev", "apps/api · pnpm run dev", "web · pnpm run dev"]);
        assert_eq!(r.candidates[2].cwd, t.0.join("web").to_string_lossy());
        assert_eq!(r.candidates[2].source, "web/package.json");
    }

    #[test]
    fn lifecycle_hooks_are_skipped() {
        let t = TempDir::new();
        t.file("package.json", r#"{"scripts":{"predev:api":"sync","dev:api":"node api","prepare":"husky"}}"#);
        assert_eq!(commands(&scan(&t.0).unwrap()), ["npm run dev:api", "npm run prepare"]);
    }

    #[test]
    fn script_files_are_found_and_ranked() {
        let t = TempDir::new();
        t.file("start-local.cmd", "@echo off")
            .file("Run-local.ps1", "")
            .file("stop_app.bat", "")
            .file("Facer.bat", "")
            .file("Run-local.sh", "")
            .file("README.md", "");
        let r = scan(&t.0).unwrap();
        let titles: Vec<_> = r.candidates.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Run-local.ps1", "start-local.cmd", "Facer.bat", "stop_app.bat"]);
        assert!(r.candidates[1].recommended && !r.candidates[2].recommended);
        assert_eq!(r.candidates[1].kind, KIND_SCRIPT);
        assert_eq!(r.candidates[1].command, t.0.join("start-local.cmd").to_string_lossy());
    }

    #[test]
    fn start_lines_are_understood() {
        let direct = |line| parse_start(line).and_then(|w| w.direct);
        let opens_window = |line| parse_start(line).is_some();

        // Сервер в своём окне: команду можно запускать напрямую.
        assert_eq!(
            direct(r#"start "AIRETOUCHER server" cmd /k ".venv\Scripts\python.exe webui.py""#).as_deref(),
            Some(r".venv\Scripts\python.exe webui.py")
        );
        assert_eq!(
            direct(r#"START "api" /D "C:\my app" node server.js --port 3000"#).as_deref(),
            Some("node server.js --port 3000")
        );
        assert_eq!(
            direct(r#"@start "" "C:\tools\python.exe" app.py"#).as_deref(),
            Some(r#""C:\tools\python.exe" app.py"#)
        );

        // Окно есть, но это не сервер, а ожидание с открытием браузера.
        let helper = r#"start "open-browser" /min cmd /c "for /l %%i in (1,1,90) do (curl -sf http://127.0.0.1:7430/ && start http://localhost:7430/)""#;
        assert!(opens_window(helper));
        assert_eq!(direct(helper), None);

        // Нового окна консоли не будет.
        assert!(!opens_window(r#"start "" /b cmd /c "ping -n 5 127.0.0.1 >nul && start http://127.0.0.1:7440""#));
        assert!(!opens_window(r#"start "" "http://127.0.0.1:7860""#));
        assert!(!opens_window(r#"start "" http://localhost:5290"#));
        assert!(!opens_window(r#"start "" "C:\Program Files\App\viewer.exe""#));
        assert!(!opens_window("echo start the server"));
        assert!(!opens_window("starter.exe run"));
    }

    #[test]
    fn script_with_own_window_offers_its_inner_command() {
        let t = TempDir::new();
        t.file(
            "start_webui.bat",
            "@echo off\r\ncd /d \"%~dp0\"\r\nstart \"AIRETOUCHER server\" cmd /k \".venv\\Scripts\\python.exe webui.py\"\r\nstart \"\" \"http://127.0.0.1:7860\"\r\n",
        )
        .file("quiet.cmd", "@echo off\r\nstart \"\" /b node server.js\r\n");
        let r = scan(&t.0).unwrap();
        let titles: Vec<_> = r.candidates.iter().map(|c| c.title.as_str()).collect();
        // Прямая команда стоит сразу над файлом, из которого взята.
        assert_eq!(titles, [".venv\\Scripts\\python.exe webui.py", "start_webui.bat", "quiet.cmd"]);

        let inner = &r.candidates[0];
        assert_eq!((inner.kind.as_str(), inner.source.as_str()), (KIND_COMMAND, "из start_webui.bat"));
        assert_eq!(inner.cwd, t.0.to_string_lossy());
        assert!(inner.recommended && inner.warning.is_none());

        assert!(r.candidates[1].warning.as_ref().unwrap().contains("отдельное окно"));
        assert!(r.candidates[2].warning.is_none());
    }

    #[test]
    fn root_dev_script_outranks_wrapper_file() {
        let t = TempDir::new();
        t.file("package.json", r#"{"scripts":{"dev":"node server"}}"#).file("start.cmd", "");
        let r = scan(&t.0).unwrap();
        assert_eq!(r.candidates[0].command, "npm run dev");
        assert_eq!(r.candidates[1].title, "start.cmd");
    }

    #[test]
    fn empty_folder_gives_no_candidates() {
        let t = TempDir::new();
        let r = scan(&t.0).unwrap();
        assert!(r.candidates.is_empty());
        assert_eq!(r.path, t.0.to_string_lossy());
    }

    #[test]
    fn broken_package_json_is_not_fatal() {
        let t = TempDir::new();
        t.file("package.json", "{ oops").file("dev.cmd", "");
        assert_eq!(scan(&t.0).unwrap().candidates.len(), 1);
    }

    #[test]
    fn missing_folder_is_an_error() {
        assert!(scan(Path::new("C:\\definitely\\not\\here\\lsm")).is_err());
    }
}
