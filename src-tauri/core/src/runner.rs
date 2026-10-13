//! Запуск серверов, их статусы и вывод терминала.
//!
//! Каждый сервер работает в собственной псевдоконсоли внутри Job Object. От интерфейса модуль
//! не зависит: о переменах он сообщает через [`Sink`], поэтому весь жизненный цикл процесса
//! проверяется тестами без окна.

use crate::{
    config::KIND_SCRIPT,
    freeze,
    job::Job,
    ports,
    pty::{self, Pty},
    text::{self, Utf8Stream},
};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Read,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Сколько вывод копится перед отправкой в интерфейс. Достаточно мало, чтобы эхо набранной
/// клавиши не запаздывало, и достаточно много, чтобы шумная сборка не слала событие на каждый байт.
const OUTPUT_COALESCE: Duration = Duration::from_millis(12);
/// Как часто сверять процессы серверов с таблицей слушающих портов.
const PORT_SCAN: Duration = Duration::from_secs(1);
/// Процесс без порта (наблюдатель за файлами, очередь задач) считаем поднявшимся через это время.
const READY_WITHOUT_PORT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(40);
const JOB_POLL: Duration = Duration::from_millis(250);
const STOP_TIMEOUT: Duration = Duration::from_secs(8);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// Длина пояснения к статусу в строке сервера.
const NOTE_CHARS: usize = 200;
/// Сколько последних строк вывода просматривать в поисках причины завершения.
const TAIL_LINES: usize = 12;
/// Предел недописанной строки: вывод без переводов строки не должен копиться бесконечно.
const LINE_BYTES: usize = 16 * 1024;
/// Сколько байт вывода хранить на одну строку прокрутки терминала.
const BYTES_PER_LINE: usize = 160;
/// Размер терминала, пока интерфейс не сообщил настоящий.
const DEFAULT_SIZE: (u16, u16) = (120, 30);
/// Код выхода процесса, прерванного Ctrl+C: это остановка рукой пользователя, а не падение.
const STATUS_CONTROL_C_EXIT: i32 = 0xC000_013A_u32 as i32;
/// Сколько после Ctrl+C в терминале завершение процесса считается делом рук пользователя.
/// Срок нужен на ответ пакетному файлу «Завершить выполнение? (Y/N)»: тот выходит с кодом 255,
/// и без этой поправки остановка по Ctrl+C выглядела бы падением.
const INTERRUPT_GRACE: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Stopped,
    Starting,
    Running,
    /// Процессы заморожены: не тратят процессор, но помнят всё и продолжат с того же места.
    Paused,
    /// Завершился сам с ненулевым кодом.
    Crashed,
    /// Завершился сам с кодом 0: не ошибка, но и не остановка по кнопке. Так выходит,
    /// например, сервер, который обнаружил, что его порт уже занят другой копией.
    Exited,
}

impl Status {
    /// Процессы сервера существуют: он запускается, работает или стоит на паузе.
    pub fn is_alive(self) -> bool {
        matches!(self, Status::Starting | Status::Running | Status::Paused)
    }

    /// Сервер выполняется прямо сейчас, а не заморожен.
    pub fn is_running(self) -> bool {
        matches!(self, Status::Starting | Status::Running)
    }
}

/// Кусок вывода терминала: текст с управляющими последовательностями, как его выдала консоль.
#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    /// Порядковый номер в пределах сервера: по нему интерфейс отбрасывает повторы.
    pub n: u64,
    pub s: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Output {
    pub id: String,
    pub chunks: Vec<Chunk>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcView {
    pub id: String,
    pub status: Status,
    pub pid: Option<u32>,
    pub started_at: Option<u64>,
    pub exit_code: Option<i32>,
    pub url: Option<String>,
    pub ports: Vec<u16>,
    /// Строка вывода процесса, который завершился сам: обычно в ней и причина.
    pub note: Option<String>,
}

/// Что запускать: срез способа запуска из настроек.
#[derive(Clone, Debug)]
pub struct Spec {
    pub id: String,
    pub kind: String,
    pub command: String,
    pub cwd: String,
}

pub trait Sink: Send + Sync + 'static {
    fn procs(&self, views: Vec<ProcView>);
    fn output(&self, batches: Vec<Output>);
    fn crashed(&self, id: &str, code: i32);
}

#[derive(Default)]
struct Proc {
    status: Status,
    pid: Option<u32>,
    job: Option<Arc<Job>>,
    pty: Option<Arc<Pty>>,
    started_at: Option<u64>,
    started: Option<Instant>,
    exit_code: Option<i32>,
    /// Адрес, найденный в выводе процесса.
    url: Option<String>,
    ports: Vec<u16>,
    /// Вывод терминала за все запуски: по нему интерфейс восстанавливает экран при открытии.
    out: VecDeque<Chunk>,
    out_bytes: usize,
    seq: u64,
    /// Недописанная строка вывода: адрес и причина завершения ищутся по целым строкам.
    line: String,
    /// Последние непустые строки вывода текущего запуска обычным текстом.
    tail: VecDeque<String>,
    /// Номер запуска. Потоки прошлого запуска узнают по нему, что их время вышло.
    run: u64,
    stopping: bool,
    /// Когда пользователь последний раз нажал Ctrl+C в терминале этого запуска.
    interrupted: Option<Instant>,
    /// Процессы, замороженные паузой, и статус, в который вернуться после неё.
    frozen: Vec<u32>,
    before_pause: Status,
}

pub struct Runner {
    sink: Arc<dyn Sink>,
    procs: Mutex<HashMap<String, Proc>>,
    pending: Mutex<Vec<(String, Chunk)>>,
    wake: Condvar,
    out_cap: AtomicUsize,
    size: Mutex<(u16, u16)>,
}

/// Командная строка, с которой запускается сервер.
///
/// С ключом `/s` cmd снимает первую и последнюю кавычку, а остальное выполняет как есть,
/// поэтому путь с пробелами остаётся в собственных кавычках.
pub fn command_line(kind: &str, command: &str) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("пустая команда".into());
    }
    let via_cmd = |inner: String| format!("cmd.exe /d /s /c \"{inner}\"");
    if kind != KIND_SCRIPT {
        return Ok(via_cmd(command.to_string()));
    }

    let quoted = format!("\"{command}\"");
    let ext = Path::new(command)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    Ok(match ext.as_str() {
        "ps1" => format!("powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File {quoted}"),
        "exe" => quoted,
        // Файл без «исполняемого» расширения cmd открыл бы в редакторе по ассоциации.
        "js" | "mjs" | "cjs" => via_cmd(format!("node {quoted}")),
        "py" => via_cmd(format!("python {quoted}")),
        _ => via_cmd(quoted),
    })
}

/// Строка самого приложения в потоке терминала: «Запуск», «Остановлен».
fn mark(text: &str) -> String {
    format!("\x1b[0m\x1b[36m▸ {text}\x1b[0m")
}

impl Runner {
    pub fn new(sink: Arc<dyn Sink>, scrollback_lines: usize) -> Arc<Runner> {
        Arc::new(Runner {
            sink,
            procs: Mutex::new(HashMap::new()),
            pending: Mutex::new(Vec::new()),
            wake: Condvar::new(),
            out_cap: AtomicUsize::new(scrollback_lines * BYTES_PER_LINE),
            size: Mutex::new(DEFAULT_SIZE),
        })
    }

    pub fn set_scrollback(&self, lines: usize) {
        self.out_cap.store(lines * BYTES_PER_LINE, Ordering::Relaxed);
    }

    /// Фоновый поток: отдаёт накопленный вывод интерфейсу и следит за портами.
    pub fn spawn_ticker(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        thread::spawn(move || {
            let mut scanned = Instant::now();
            loop {
                let Some(runner) = weak.upgrade() else { break };
                let has_output = {
                    let pending = runner.pending.lock().unwrap_or_else(|e| e.into_inner());
                    let (pending, _) = runner
                        .wake
                        .wait_timeout_while(pending, PORT_SCAN, |p| p.is_empty())
                        .unwrap_or_else(|e| e.into_inner());
                    !pending.is_empty()
                };
                if has_output {
                    thread::sleep(OUTPUT_COALESCE);
                    runner.flush_output();
                }
                if scanned.elapsed() >= PORT_SCAN {
                    scanned = Instant::now();
                    runner.scan_ports();
                }
            }
        });
    }

    pub fn start(self: &Arc<Self>, spec: &Spec) -> Result<(), String> {
        let run = {
            let mut procs = self.lock();
            let p = procs.entry(spec.id.clone()).or_default();
            if p.status.is_alive() {
                return Ok(());
            }
            p.run += 1;
            p.status = Status::Starting;
            p.stopping = false;
            p.interrupted = None;
            p.frozen.clear();
            p.exit_code = None;
            p.url = None;
            p.ports.clear();
            p.tail.clear();
            p.line.clear();
            p.started_at = Some(now_ms());
            p.started = Some(Instant::now());
            p.run
        };
        let (cols, rows) = self.size();
        // Новая консоль первым делом очищает экран. Чтобы прошлый запуск не пропал, сдвигаем
        // его в прокрутку пустыми строками и ставим курсор в начало: так экран терминала
        // и представление консоли о нём совпадают с первой же строки.
        let scroll = "\r\n".repeat(rows as usize + 1);
        let banner = mark(&format!("Запуск: {}", spec.command));
        self.push_output(&spec.id, run, format!("\x1b[?25h{banner}{scroll}\x1b[H"), false);

        let result = self.spawn(spec, run, cols, rows);
        if let Err(e) = &result {
            if let Some(p) = self.lock().get_mut(&spec.id).filter(|p| p.run == run) {
                p.status = Status::Crashed;
                p.pid = None;
                p.job = None;
                p.pty = None;
                p.started_at = None;
                p.started = None;
                p.tail.push_back(text::capitalize(e.clone()));
            }
            let failed = mark(&format!("Не удалось запустить: {e}"));
            self.push_output(&spec.id, run, format!("{failed}\r\n"), false);
        }
        self.notify();
        result
    }

    fn spawn(self: &Arc<Self>, spec: &Spec, run: u64, cols: u16, rows: u16) -> Result<(), String> {
        if !Path::new(&spec.cwd).is_dir() {
            return Err(format!("папка не найдена: {}", spec.cwd));
        }
        let line = command_line(&spec.kind, &spec.command)?;
        let job = Arc::new(Job::new().map_err(|e| format!("не удалось создать задание: {e}"))?);
        let spawned = pty::spawn(&line, &spec.cwd, cols, rows, &job).map_err(|e| e.to_string())?;
        let pty = Arc::new(spawned.pty);
        let process = spawned.process;

        // Остановить могли успеть, пока процесс создавался: тогда задания у записи ещё не было.
        let stop_now = match self.lock().get_mut(&spec.id).filter(|p| p.run == run) {
            Some(p) => {
                p.pid = Some(process.pid);
                p.job = Some(job.clone());
                p.pty = Some(pty.clone());
                p.stopping
            }
            None => true,
        };
        if stop_now {
            let _ = job.terminate();
        }

        let (drained, reader) = mpsc::channel();
        {
            let runner = self.clone();
            let id = spec.id.clone();
            let mut output = spawned.output;
            thread::spawn(move || {
                let mut decoder = Utf8Stream::default();
                let mut buf = [0u8; 8192];
                loop {
                    match output.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let text = decoder.feed(&buf[..n]);
                            if !text.is_empty() {
                                runner.push_output(&id, run, text, true);
                            }
                        }
                    }
                }
                let _ = drained.send(());
            });
        }

        let runner = self.clone();
        let id = spec.id.clone();
        thread::spawn(move || {
            let code = process.wait();
            // Корневой процесс вышел, но сервер мог остаться жить в задании: скрипт поднял его
            // через `start` и завершился. Сервер остановлен, только когда задание опустело.
            while !job.pids().is_empty() {
                thread::sleep(JOB_POLL);
            }
            drop(job);
            // У вывода консоли нет конца, пока она открыта. Закрываем и дочитываем остаток,
            // чтобы итоговая строка «Остановлен» шла последней. Ждём с пределом: статус
            // не должен зависнуть, если консоль не закроется.
            pty.close();
            let _ = reader.recv_timeout(DRAIN_TIMEOUT);
            runner.finish(&id, run, code);
        });
        Ok(())
    }

    /// Принимает кусок вывода. `scan` — вывод самого процесса: в нём ищутся адрес сервера
    /// и строки, объясняющие завершение. Собственные отметки приложения не просматриваются.
    fn push_output(&self, id: &str, run: u64, text: String, scan: bool) {
        let cap = self.out_cap.load(Ordering::Relaxed);
        let (chunk, changed) = {
            let mut procs = self.lock();
            let Some(p) = procs.get_mut(id).filter(|p| p.run == run) else { return };
            p.seq += 1;
            let chunk = Chunk { n: p.seq, s: text };
            p.out_bytes += chunk.s.len();
            p.out.push_back(chunk.clone());
            while p.out_bytes > cap && p.out.len() > 1 {
                if let Some(old) = p.out.pop_front() {
                    p.out_bytes -= old.s.len();
                }
            }

            let mut changed = false;
            if scan {
                p.line.push_str(&chunk.s);
                while let Some(end) = p.line.find('\n') {
                    let raw: String = p.line.drain(..=end).collect();
                    let plain = text::plain_line(&raw);
                    if plain.is_empty() {
                        continue;
                    }
                    if p.url.is_none() && p.status.is_alive() {
                        if let Some(url) = text::find_local_url(&plain) {
                            p.url = Some(url);
                            changed = true;
                        }
                    }
                    p.tail.push_back(text::shorten(&plain, NOTE_CHARS));
                    while p.tail.len() > TAIL_LINES {
                        p.tail.pop_front();
                    }
                }
                if p.line.len() > LINE_BYTES {
                    p.line.clear();
                }
            }
            (chunk, changed)
        };
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).push((id.to_string(), chunk));
        self.wake.notify_one();
        if changed {
            self.notify();
        }
    }

    fn finish(&self, id: &str, run: u64, code: i32) {
        let (crashed, message) = {
            let mut procs = self.lock();
            let Some(p) = procs.get_mut(id).filter(|p| p.run == run) else { return };
            // Ctrl+C в терминале — такая же остановка рукой пользователя, как кнопка.
            let interrupted = p.interrupted.is_some_and(|at| at.elapsed() < INTERRUPT_GRACE);
            let by_user = p.stopping || interrupted || code == STATUS_CONTROL_C_EXIT;
            let crashed = !by_user && code != 0;
            p.status = if by_user {
                Status::Stopped
            } else if crashed {
                Status::Crashed
            } else {
                Status::Exited
            };
            p.stopping = false;
            p.frozen.clear();
            p.pid = None;
            p.job = None;
            p.pty = None;
            p.started_at = None;
            p.started = None;
            p.url = None;
            p.ports.clear();
            p.exit_code = Some(code);
            let message = if by_user {
                "Остановлен".to_string()
            } else if crashed {
                format!("Процесс упал, код выхода {code}")
            } else {
                "Процесс завершился сам, код выхода 0".to_string()
            };
            (crashed, message)
        };
        // Курсор после завершения прячем: вводить больше некуда.
        let last = mark(&message);
        self.push_output(id, run, format!("\r\n{last}\r\n\x1b[?25l"), false);
        self.notify();
        if crashed {
            self.sink.crashed(id, code);
        }
    }

    /// Убивает всё дерево процессов сервера. Не ждёт: статус сменится, когда задание опустеет.
    pub fn stop(&self, id: &str) {
        let job = {
            let mut procs = self.lock();
            let Some(p) = procs.get_mut(id).filter(|p| p.status.is_alive()) else { return };
            p.stopping = true;
            p.job.clone()
        };
        if let Some(job) = job {
            let _ = job.terminate();
        }
    }

    /// Ставит сервер на паузу: замораживает все его процессы и вытесняет их память в подкачку.
    /// Порты остаются заняты, запросы к серверу будут ждать продолжения.
    pub fn pause(&self, id: &str) -> Result<(), String> {
        let (job, run) = {
            let procs = self.lock();
            let p = procs.get(id).filter(|p| p.status.is_running()).ok_or("сервер не запущен")?;
            (p.job.clone().ok_or("сервер ещё запускается")?, p.run)
        };
        // Заморозка идёт без блокировки: она обходит все процессы дерева и может занять время.
        let frozen = freeze::suspend(&job);
        if frozen.is_empty() {
            return Err("не удалось приостановить процессы сервера".into());
        }
        {
            let mut procs = self.lock();
            match procs.get_mut(id).filter(|p| p.run == run && p.status.is_running()) {
                Some(p) => {
                    p.before_pause = p.status;
                    p.status = Status::Paused;
                    p.frozen = frozen;
                }
                // Пока замораживали, сервер остановили или он завершился: возвращаем как было.
                None => {
                    freeze::resume(&frozen);
                    return Err("сервер уже остановлен".into());
                }
            }
        }
        self.notify();
        Ok(())
    }

    /// Снимает сервер с паузы.
    pub fn resume(&self, id: &str) -> Result<(), String> {
        let frozen = {
            let mut procs = self.lock();
            let p = procs.get_mut(id).filter(|p| p.status == Status::Paused).ok_or("сервер не на паузе")?;
            p.status = p.before_pause;
            // Отсчёт «не дождались порта» начинается заново: время на паузе в него не входит.
            p.started = Some(Instant::now());
            std::mem::take(&mut p.frozen)
        };
        freeze::resume(&frozen);
        self.notify();
        Ok(())
    }

    pub fn stop_wait(&self, id: &str, timeout: Duration) -> bool {
        self.stop(id);
        let deadline = Instant::now() + timeout;
        while self.is_alive(id) {
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
        true
    }

    pub fn restart(self: &Arc<Self>, spec: &Spec) -> Result<(), String> {
        if !self.stop_wait(&spec.id, STOP_TIMEOUT) {
            return Err("процесс не остановился, перезапуск отменён".into());
        }
        self.start(spec)
    }

    pub fn stop_all(&self) {
        for id in self.active_ids() {
            self.stop(&id);
        }
    }

    /// Остановка перед выходом из приложения: ждём недолго, остальное добьёт закрытие заданий.
    pub fn shutdown(&self) {
        self.stop_all();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.active_ids().is_empty() && Instant::now() < deadline {
            thread::sleep(POLL);
        }
    }

    /// Способ запуска удалён из настроек: останавливаем и забываем его вывод.
    pub fn forget(&self, id: &str) {
        self.stop(id);
        // Задание закрывается вместе с записью, так что процессы не переживут её в любом случае.
        self.lock().remove(id);
        self.notify();
    }

    /// Ввод с клавиатуры в терминал сервера.
    pub fn input(&self, id: &str, data: &str) {
        let pty = {
            let mut procs = self.lock();
            // Замороженный процесс ввод не прочитает: клавиши, набранные на паузе, не копим.
            let Some(p) = procs.get_mut(id).filter(|p| p.status.is_running()) else { return };
            if data.contains('\x03') {
                p.interrupted = Some(Instant::now());
            }
            p.pty.clone()
        };
        if let Some(pty) = pty {
            let _ = pty.write(data.as_bytes());
        }
    }

    /// Интерфейс сообщает размер окна терминала. Он один на все серверы: окно терминала одно.
    pub fn resize(&self, cols: u16, rows: u16) {
        {
            let mut size = self.size.lock().unwrap_or_else(|e| e.into_inner());
            if *size == (cols, rows) {
                return;
            }
            *size = (cols, rows);
        }
        let ptys: Vec<Arc<Pty>> = self.lock().values().filter_map(|p| p.pty.clone()).collect();
        for pty in ptys {
            pty.resize(cols, rows);
        }
    }

    fn size(&self) -> (u16, u16) {
        *self.size.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_alive(&self, id: &str) -> bool {
        self.lock().get(id).is_some_and(|p| p.status.is_alive())
    }

    pub fn active_ids(&self) -> Vec<String> {
        self.lock().iter().filter(|(_, p)| p.status.is_alive()).map(|(id, _)| id.clone()).collect()
    }

    /// Какому серверу принадлежит каждый процесс: для вкладки портов.
    pub fn owners(&self) -> HashMap<u32, String> {
        let jobs: Vec<(String, Arc<Job>)> =
            self.lock().iter().filter_map(|(id, p)| Some((id.clone(), p.job.clone()?))).collect();
        let mut owners = HashMap::new();
        for (id, job) in jobs {
            for pid in job.pids() {
                owners.insert(pid, id.clone());
            }
        }
        owners
    }

    pub fn snapshot(&self) -> Vec<ProcView> {
        self.lock()
            .iter()
            .map(|(id, p)| ProcView {
                id: id.clone(),
                status: p.status,
                pid: p.pid,
                started_at: p.started_at,
                exit_code: p.exit_code,
                // Адрес из вывода точнее: у сервера с несколькими портами он указывает на главный.
                url: p.url.clone().or_else(|| p.ports.first().map(|port| format!("http://localhost:{port}"))),
                ports: p.ports.clone(),
                note: if matches!(p.status, Status::Crashed | Status::Exited) { text::pick_reason(&p.tail) } else { None },
            })
            .collect()
    }

    /// Весь сохранённый вывод сервера: с него терминал начинает, когда его открывают.
    pub fn output(&self, id: &str) -> Vec<Chunk> {
        self.lock().get(id).map(|p| p.out.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn clear_output(&self, id: &str) {
        if let Some(p) = self.lock().get_mut(id) {
            p.out.clear();
            p.out_bytes = 0;
        }
    }

    fn flush_output(&self) {
        let pending = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        if pending.is_empty() {
            return;
        }
        let mut batches: Vec<Output> = Vec::new();
        for (id, chunk) in pending {
            match batches.iter_mut().find(|b| b.id == id) {
                Some(batch) => batch.chunks.push(chunk),
                None => batches.push(Output { id, chunks: vec![chunk] }),
            }
        }
        self.sink.output(batches);
    }

    /// Сверяет процессы каждого задания с таблицей слушающих сокетов.
    /// Сервер считается поднявшимся, когда кто-то из его процессов начал слушать порт.
    fn scan_ports(&self) {
        let active: Vec<(String, u64, Arc<Job>)> = self
            .lock()
            .iter()
            .filter(|(_, p)| p.status.is_alive())
            .filter_map(|(id, p)| Some((id.clone(), p.run, p.job.clone()?)))
            .collect();
        if active.is_empty() {
            return;
        }

        let table = ports::listening();
        let mut changed = false;
        for (id, run, job) in active {
            let pids: HashSet<u32> = job.pids().into_iter().collect();
            let mut found: Vec<u16> =
                table.iter().filter(|(pid, _)| pids.contains(pid)).map(|&(_, port)| port).collect();
            found.sort_unstable();
            found.dedup();

            let mut procs = self.lock();
            let Some(p) = procs.get_mut(&id).filter(|p| p.run == run && p.status.is_alive()) else { continue };
            if p.ports != found {
                p.ports = found;
                changed = true;
            }
            let waited = p.started.is_some_and(|t| t.elapsed() >= READY_WITHOUT_PORT);
            if p.status == Status::Starting && (!p.ports.is_empty() || waited) {
                p.status = Status::Running;
                changed = true;
            }
        }
        if changed {
            self.notify();
        }
    }

    /// Сообщает о перемене. Вызывать только без захваченной блокировки: получатель события
    /// читает состояние обратно.
    fn notify(&self) {
        self.sink.procs(self.snapshot());
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Proc>> {
        // Отравленная блокировка означает панику в другом потоке; данные при этом целы,
        // а отказ управлять серверами из-за неё был бы хуже.
        self.procs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{new_id, KIND_COMMAND};
    use std::{fs, net::TcpStream, path::PathBuf};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };

    #[derive(Default)]
    struct TestSink {
        crashed: Mutex<Vec<(String, i32)>>,
        batches: Mutex<usize>,
    }

    impl Sink for TestSink {
        fn procs(&self, _views: Vec<ProcView>) {}
        fn output(&self, batches: Vec<Output>) {
            *self.batches.lock().unwrap() += batches.len();
        }
        fn crashed(&self, id: &str, code: i32) {
            self.crashed.lock().unwrap().push((id.to_string(), code));
        }
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            // Пробел в пути намеренно: половина репозиториев лежит в папках вроде «SZ Learn».
            let dir = std::env::temp_dir().join(format!("lsm run {}", new_id()));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn script(&self, name: &str, body: &str) -> String {
            let path = self.0.join(name);
            fs::write(&path, format!("@echo off\r\n{body}\r\n")).unwrap();
            path.to_string_lossy().into_owned()
        }
        fn spec(&self, kind: &str, command: &str) -> Spec {
            Spec { id: "t".into(), kind: kind.into(), command: command.into(), cwd: self.0.to_string_lossy().into_owned() }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn runner() -> (Arc<Runner>, Arc<TestSink>) {
        let sink = Arc::new(TestSink::default());
        let runner = Runner::new(sink.clone(), 1000);
        runner.spawn_ticker();
        (runner, sink)
    }

    fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while !check() {
            assert!(Instant::now() < deadline, "не дождались: {what}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn view(runner: &Runner) -> ProcView {
        runner.snapshot().into_iter().find(|v| v.id == "t").unwrap()
    }

    /// Всё, что сервер вывел в терминал, обычным текстом.
    fn screen(runner: &Runner) -> String {
        let raw: String = runner.output("t").into_iter().map(|c| c.s).collect();
        text::strip_ansi(&raw)
    }

    fn job_pids(runner: &Runner) -> Vec<u32> {
        runner.lock().get("t").and_then(|p| p.job.clone()).map(|j| j.pids()).unwrap_or_default()
    }

    /// Процессы задания, взятые «за руку» до остановки.
    ///
    /// Проверять после остановки по одному номеру процесса нельзя: Windows выдаёт освободившиеся
    /// номера заново почти сразу, а соседние тесты плодят процессы десятками, и «выживший»
    /// оказывается чужим новичком с тем же номером. Открытый дескриптор держит именно тот
    /// процесс, который был в задании, и не даёт переиспользовать его номер.
    struct Watched(Vec<(u32, HANDLE)>);

    impl Watched {
        fn job_of(runner: &Runner) -> Watched {
            // SAFETY: дескрипторы закрываются в Drop.
            let opened: Vec<(u32, HANDLE)> = job_pids(runner)
                .into_iter()
                .map(|pid| (pid, unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) }))
                .filter(|(_, handle)| !handle.is_null())
                .collect();
            // Номер, который всё ещё числится в задании при открытом дескрипторе, точно наш.
            let still = job_pids(runner);
            let (ours, strangers): (Vec<_>, Vec<_>) = opened.into_iter().partition(|(pid, _)| still.contains(pid));
            for (_, handle) in strangers {
                unsafe { CloseHandle(handle) };
            }
            Watched(ours)
        }

        fn alive(&self) -> Vec<u32> {
            self.0
                .iter()
                .filter(|(_, handle)| unsafe { WaitForSingleObject(*handle, 0) } == WAIT_TIMEOUT)
                .map(|(pid, _)| *pid)
                .collect()
        }
    }

    impl Drop for Watched {
        fn drop(&mut self) {
            for (_, handle) in &self.0 {
                unsafe { CloseHandle(*handle) };
            }
        }
    }

    const IDLE: &str = "setInterval(function(){},1000)";

    #[test]
    fn command_lines() {
        let line = |kind, cmd| command_line(kind, cmd).unwrap();
        assert_eq!(line("npm", "pnpm run dev"), "cmd.exe /d /s /c \"pnpm run dev\"");
        assert_eq!(line(KIND_COMMAND, "node server.js --port 3000"), "cmd.exe /d /s /c \"node server.js --port 3000\"");
        assert_eq!(line(KIND_SCRIPT, "C:\\Dev\\SZ Learn\\dev.cmd"), "cmd.exe /d /s /c \"\"C:\\Dev\\SZ Learn\\dev.cmd\"\"");
        assert_eq!(
            line(KIND_SCRIPT, "C:\\Dev\\SZ-CRM\\Run-local.PS1"),
            "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"C:\\Dev\\SZ-CRM\\Run-local.PS1\""
        );
        assert_eq!(line(KIND_SCRIPT, "C:\\x\\run.mjs"), "cmd.exe /d /s /c \"node \"C:\\x\\run.mjs\"\"");
        assert_eq!(line(KIND_SCRIPT, "C:\\my tools\\app.exe"), "\"C:\\my tools\\app.exe\"");
        assert!(command_line("npm", "   ").is_err());
    }

    #[test]
    fn output_and_exit_code_are_captured() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        runner.start(&t.spec(KIND_COMMAND, "echo hello&& echo oops 1>&2&& exit 3")).unwrap();
        wait_for("выход процесса", Duration::from_secs(15), || !runner.is_alive("t"));

        let v = view(&runner);
        assert_eq!(v.status, Status::Crashed);
        assert_eq!(v.exit_code, Some(3));
        assert_eq!(v.pid, None);

        let text = screen(&runner);
        assert!(text.contains("Запуск: echo hello"), "{text:?}");
        // В терминале обычный вывод и вывод ошибок идут одним потоком.
        assert!(text.contains("hello") && text.contains("oops"), "{text:?}");
        assert!(text.contains("код выхода 3"), "{text:?}");
        assert_eq!(*sink.crashed.lock().unwrap(), [("t".to_string(), 3)]);
        wait_for("отправка вывода", Duration::from_secs(2), || *sink.batches.lock().unwrap() > 0);
    }

    #[test]
    fn clean_exit_is_not_a_crash() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        runner.start(&t.spec(KIND_COMMAND, "echo done")).unwrap();
        wait_for("выход процесса", Duration::from_secs(15), || !runner.is_alive("t"));
        // Вышел сам с кодом 0: это не «остановлен кнопкой» и не «упал».
        assert_eq!(view(&runner).status, Status::Exited);
        assert_eq!(view(&runner).exit_code, Some(0));
        assert_eq!(view(&runner).note.as_deref(), Some("done"));
        assert!(sink.crashed.lock().unwrap().is_empty());
    }

    #[test]
    fn process_sees_a_real_terminal() {
        let (runner, _) = runner();
        let t = TempDir::new();
        runner.resize(100, 40);
        runner
            .start(&t.spec(KIND_COMMAND, "node -e \"console.log('tty', process.stdout.isTTY, process.stdin.isTTY, process.stdout.columns)\""))
            .unwrap();
        wait_for("выход процесса", Duration::from_secs(20), || !runner.is_alive("t"));
        assert!(screen(&runner).contains("tty true true 100"), "{:?}", screen(&runner));
    }

    #[test]
    fn script_waits_at_pause_for_a_key() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let script = t.script("with pause.cmd", "echo before\r\npause\r\necho after");
        runner.start(&t.spec(KIND_SCRIPT, &script)).unwrap();
        wait_for("строка до pause", Duration::from_secs(15), || screen(&runner).contains("before"));

        // В терминале pause ждёт клавишу, как в обычном окне консоли.
        thread::sleep(Duration::from_millis(700));
        assert!(runner.is_alive("t"));
        assert!(!screen(&runner).contains("after"));

        runner.input("t", "\r");
        wait_for("выход после нажатия клавиши", Duration::from_secs(15), || !runner.is_alive("t"));
        assert!(screen(&runner).contains("after"));
        assert_eq!(view(&runner).exit_code, Some(0));
    }

    #[test]
    fn typed_input_reaches_the_process() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let script = t.script("ask.cmd", "set /p NAME=name? \r\necho hi %NAME%!");
        runner.start(&t.spec(KIND_SCRIPT, &script)).unwrap();
        wait_for("вопрос скрипта", Duration::from_secs(15), || screen(&runner).contains("name?"));
        runner.input("t", "Maxat\r");
        wait_for("выход скрипта", Duration::from_secs(15), || !runner.is_alive("t"));
        assert!(screen(&runner).contains("hi Maxat!"), "{:?}", screen(&runner));
    }

    #[test]
    fn ctrl_c_in_terminal_is_a_stop_not_a_crash() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        // Слово собирается из двух половин: целиком оно не должно встретиться в строке «Запуск: …»,
        // иначе проверка сработает раньше, чем node вообще стартует, и Ctrl+C уйдёт в пустоту.
        runner.start(&t.spec(KIND_COMMAND, &format!("node -e \"console.log('lis'+'tening');{IDLE}\""))).unwrap();
        wait_for("сервер поднялся", Duration::from_secs(20), || screen(&runner).contains("listening"));
        runner.input("t", "\x03");
        wait_for("выход по Ctrl+C", Duration::from_secs(15), || !runner.is_alive("t"));
        assert_eq!(view(&runner).status, Status::Stopped, "код {:?}", view(&runner).exit_code);
        assert!(sink.crashed.lock().unwrap().is_empty());
    }

    #[test]
    fn ctrl_c_on_batch_file_is_a_stop_too() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        // Как `npm run dev`: пакетный файл, внутри которого работает node.
        let script = t.script("serve.cmd", &format!("node -e \"console.log('lis'+'tening');{IDLE}\""));
        runner.start(&t.spec(KIND_SCRIPT, &script)).unwrap();
        wait_for("сервер поднялся", Duration::from_secs(20), || screen(&runner).contains("listening"));

        // cmd спрашивает, завершать ли пакетный файл: «…(Y/N)?» на любом языке системы.
        // Под нагрузкой (тесты идут десятками параллельно) консоль изредка пропускает первое
        // нажатие, поэтому жмём, как жал бы человек: ещё раз, если реакции нет.
        let mut pressed = Instant::now() - Duration::from_secs(10);
        wait_for("вопрос о пакетном файле", Duration::from_secs(20), || {
            if screen(&runner).contains("/N") {
                return true;
            }
            if pressed.elapsed() > Duration::from_secs(3) {
                runner.input("t", "\x03");
                pressed = Instant::now();
            }
            false
        });
        assert!(runner.is_alive("t"));
        runner.input("t", "y\r");
        wait_for("выход пакетного файла", Duration::from_secs(15), || !runner.is_alive("t"));

        let v = view(&runner);
        assert_ne!(v.exit_code, Some(0));
        assert_eq!(v.status, Status::Stopped, "код {:?}", v.exit_code);
        assert!(sink.crashed.lock().unwrap().is_empty());
    }

    #[test]
    fn powershell_script_runs_and_reports_exit_code() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let path = t.0.join("Run local.ps1");
        fs::write(&path, "Write-Output 'ps ok'\r\nexit 4\r\n").unwrap();
        runner.start(&t.spec(KIND_SCRIPT, &path.to_string_lossy())).unwrap();
        wait_for("выход скрипта PowerShell", Duration::from_secs(30), || !runner.is_alive("t"));
        assert_eq!(view(&runner).exit_code, Some(4));
        assert_eq!(view(&runner).status, Status::Crashed);
        assert!(screen(&runner).contains("ps ok"), "{:?}", screen(&runner));
    }

    #[test]
    fn cyrillic_path_and_output_survive() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let dir = t.0.join("Проект сервера");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hello.js"), "console.log('Сервер запущен')\n").unwrap();
        // Кириллица только в путях: сам .cmd читается в кодировке консоли, а не в UTF-8.
        let script = dir.join("запуск.cmd");
        fs::write(&script, "@echo off\r\nnode \"%~dp0hello.js\"\r\n").unwrap();

        let spec = Spec {
            id: "t".into(),
            kind: KIND_SCRIPT.into(),
            command: script.to_string_lossy().into_owned(),
            cwd: dir.to_string_lossy().into_owned(),
        };
        runner.start(&spec).unwrap();
        wait_for("выход скрипта", Duration::from_secs(20), || !runner.is_alive("t"));
        assert_eq!(view(&runner).exit_code, Some(0));
        assert!(screen(&runner).contains("Сервер запущен"), "{:?}", screen(&runner));
        assert_eq!(view(&runner).note.as_deref(), Some("Сервер запущен"));
    }

    #[test]
    fn stop_kills_whole_tree() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        // cmd → node → node: как у настоящего дев-сервера с concurrently.
        let script = t.script(
            "tree.cmd",
            &format!("node -e \"require('child_process').spawn(process.execPath,['-e','{IDLE}'],{{stdio:'inherit'}});{IDLE}\""),
        );
        runner.start(&t.spec(KIND_SCRIPT, &script)).unwrap();
        wait_for("три процесса в задании", Duration::from_secs(20), || job_pids(&runner).len() >= 3);

        let tree = Watched::job_of(&runner);
        assert!(tree.0.len() >= 3, "в задании {} процессов", tree.0.len());
        assert_eq!(tree.alive().len(), tree.0.len());
        assert_eq!(runner.owners().len(), job_pids(&runner).len());

        assert!(runner.stop_wait("t", STOP_TIMEOUT));
        assert_eq!(view(&runner).status, Status::Stopped);
        assert_eq!(view(&runner).note, None);
        assert_eq!(tree.alive(), Vec::<u32>::new(), "процессы пережили остановку");
        assert!(screen(&runner).contains("Остановлен"));
        // Остановка по кнопке падением не считается.
        assert!(sink.crashed.lock().unwrap().is_empty());
    }

    #[test]
    fn listening_port_makes_server_ready_and_stop_frees_it() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let script = t.script(
            "server.cmd",
            "node -e \"var s=require('http').createServer(function(q,r){r.end('ok')});s.listen(0,'127.0.0.1',function(){console.log('ready http://127.0.0.1:'+s.address().port+'/')})\"",
        );
        let spec = t.spec(KIND_SCRIPT, &script);
        runner.start(&spec).unwrap();
        assert_eq!(view(&runner).status, Status::Starting);
        wait_for("статус «работает»", Duration::from_secs(20), || view(&runner).status == Status::Running);

        let v = view(&runner);
        assert_eq!(v.ports.len(), 1);
        let port = v.ports[0];
        wait_for("адрес из вывода", Duration::from_secs(5), || view(&runner).url.is_some());
        assert_eq!(view(&runner).url.unwrap(), format!("http://127.0.0.1:{port}/"));
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

        // Перезапуск поднимает сервер заново, на новом порту.
        runner.restart(&spec).unwrap();
        wait_for("работает после перезапуска", Duration::from_secs(20), || view(&runner).status == Status::Running);
        let again = view(&runner).ports[0];
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "старый порт остался занят");
        // Вывод прошлого запуска остаётся в прокрутке терминала.
        assert_eq!(screen(&runner).matches("Запуск:").count(), 2);

        assert!(runner.stop_wait("t", STOP_TIMEOUT));
        assert!(TcpStream::connect(("127.0.0.1", again)).is_err(), "порт занят после остановки");
        assert!(view(&runner).ports.is_empty() && view(&runner).url.is_none());
    }

    #[test]
    fn detached_child_keeps_server_running() {
        let (runner, _) = runner();
        let t = TempDir::new();
        // Скрипт поднимает сервер через `start /b` и сразу выходит: сервер при этом работает.
        let script = t.script("detached.cmd", &format!("start \"\" /b node -e \"{IDLE}\"\r\nexit /b 0"));
        runner.start(&t.spec(KIND_SCRIPT, &script)).unwrap();
        let root = view(&runner).pid.unwrap();
        wait_for("корень вышел, потомок остался", Duration::from_secs(20), || {
            let pids = job_pids(&runner);
            !pids.is_empty() && !pids.contains(&root)
        });
        assert!(runner.is_alive("t"));
        let orphans = Watched::job_of(&runner);
        assert!(!orphans.0.is_empty());

        assert!(runner.stop_wait("t", STOP_TIMEOUT));
        assert_eq!(orphans.alive(), Vec::<u32>::new(), "потомок пережил остановку");
    }

    /// Скрипт, который раз в 100 мс печатает очередной «tick».
    fn ticker(t: &TempDir) -> Spec {
        let script = t.script(
            "ticker.cmd",
            "node -e \"var n=0;setInterval(function(){console.log('ti'+'ck ',++n)},100)\"",
        );
        t.spec(KIND_SCRIPT, &script)
    }

    fn ticks(runner: &Runner) -> usize {
        screen(runner).matches("tick ").count()
    }

    #[test]
    fn pause_freezes_the_tree_and_resume_continues() {
        let (runner, _) = runner();
        let t = TempDir::new();
        runner.start(&ticker(&t)).unwrap();
        wait_for("сервер пишет", Duration::from_secs(20), || ticks(&runner) >= 3);
        let pid = view(&runner).pid;

        runner.pause("t").unwrap();
        assert_eq!(view(&runner).status, Status::Paused);
        // Заморожены все процессы дерева, а не только корневой cmd.
        // Считаем по отдельности: оба выражения берут одну и ту же блокировку.
        let frozen = runner.lock().get("t").unwrap().frozen.len();
        assert_eq!(frozen, job_pids(&runner).len());

        // Даём дойти выводу, который был в пути, и убеждаемся, что нового нет.
        thread::sleep(Duration::from_millis(400));
        let at_pause = ticks(&runner);
        thread::sleep(Duration::from_millis(900));
        assert_eq!(ticks(&runner), at_pause, "сервер продолжил писать на паузе");
        assert!(runner.is_alive("t"));
        // Повторная пауза и ввод на паузе ничего не ломают.
        assert!(runner.pause("t").is_err());
        runner.input("t", "x");

        runner.resume("t").unwrap();
        assert!(view(&runner).status.is_running());
        wait_for("сервер снова пишет", Duration::from_secs(10), || ticks(&runner) >= at_pause + 3);
        // Это тот же процесс, а не новый запуск: ничего не пересобиралось.
        assert_eq!(view(&runner).pid, pid);
        assert_eq!(screen(&runner).matches("Запуск:").count(), 1);
        assert!(runner.resume("t").is_err());

        assert!(runner.stop_wait("t", STOP_TIMEOUT));
    }

    #[test]
    fn paused_server_can_be_stopped() {
        let (runner, sink) = runner();
        let t = TempDir::new();
        runner.start(&ticker(&t)).unwrap();
        wait_for("сервер пишет", Duration::from_secs(20), || ticks(&runner) >= 2);
        runner.pause("t").unwrap();
        let tree = Watched::job_of(&runner);
        assert!(tree.0.len() >= 2);

        assert!(runner.stop_wait("t", STOP_TIMEOUT));
        assert_eq!(view(&runner).status, Status::Stopped);
        assert_eq!(tree.alive(), Vec::<u32>::new(), "замороженные процессы пережили остановку");
        assert!(sink.crashed.lock().unwrap().is_empty());
        assert!(runner.pause("t").is_err());
    }

    #[test]
    fn missing_folder_fails_to_start() {
        let (runner, _) = runner();
        let spec = Spec { id: "t".into(), kind: KIND_COMMAND.into(), command: "echo x".into(), cwd: "C:\\no\\such\\lsm".into() };
        assert!(runner.start(&spec).is_err());
        assert_eq!(view(&runner).status, Status::Crashed);
        assert!(view(&runner).note.unwrap().starts_with("Папка не найдена"));
        assert!(screen(&runner).contains("Не удалось запустить"));
    }

    #[test]
    fn missing_program_fails_to_start() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let missing = t.0.join("no such tool.exe");
        assert!(runner.start(&t.spec(KIND_SCRIPT, &missing.to_string_lossy())).is_err());
        assert_eq!(view(&runner).status, Status::Crashed);
        assert!(!runner.is_alive("t"));
    }

    #[test]
    fn second_start_while_running_is_ignored() {
        let (runner, _) = runner();
        let t = TempDir::new();
        let spec = t.spec(KIND_COMMAND, &format!("node -e \"{IDLE}\""));
        runner.start(&spec).unwrap();
        wait_for("процесс в задании", Duration::from_secs(20), || job_pids(&runner).len() >= 2);
        let pid = view(&runner).pid;
        runner.start(&spec).unwrap();
        assert_eq!(view(&runner).pid, pid);
        assert!(runner.stop_wait("t", STOP_TIMEOUT));
    }

    #[test]
    fn output_buffer_is_capped() {
        let sink = Arc::new(TestSink::default());
        // Десять строк прокрутки: буфер на полторы тысячи байт.
        let runner = Runner::new(sink, 10);
        let t = TempDir::new();
        runner.start(&t.spec(KIND_COMMAND, "for /l %i in (1,1,400) do @echo line number %i")).unwrap();
        wait_for("выход процесса", Duration::from_secs(20), || !runner.is_alive("t"));

        let chunks = runner.output("t");
        let bytes: usize = chunks.iter().map(|c| c.s.len()).sum();
        let largest = chunks.iter().map(|c| c.s.len()).max().unwrap_or(0);
        assert!(bytes <= 10 * BYTES_PER_LINE + largest, "в буфере {bytes} байт");
        assert!(chunks.windows(2).all(|w| w[0].n < w[1].n));
        // Старое вытеснено, свежее на месте.
        assert!(screen(&runner).contains("код выхода 0"));
        assert!(!screen(&runner).contains("Запуск:"));

        runner.clear_output("t");
        assert!(runner.output("t").is_empty());
    }
}
