//! Сколько ресурсов занимает сервер: процессор, оперативная память, видеокарта.
//!
//! Считается по группам: у репозитория с несколькими способами запуска расход общий. В группу
//! входят задания серверов менеджера и отдельные процессы серверов, запущенных мимо него.
//! Процессорное время отдаёт задание (или каждый процесс, если задания нет), память — каждый
//! процесс, а видеокарту — счётчики производительности Windows, те же, по которым строит
//! столбец «GPU» диспетчер задач.

use crate::job::Job;
use serde::Serialize;
use std::{
    collections::HashMap,
    mem, ptr,
    sync::Arc,
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::{
        Performance::{
            PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
            PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE,
        },
        ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

/// Загрузка движков видеокарты по процессам: `pid_1234_luid_…_phys_0_eng_3_engtype_3D`.
const GPU_ENGINE: &str = r"\GPU Engine(*)\Utilization Percentage";
/// Выделенная видеопамять по процессам: `pid_1234_luid_…_phys_0`.
const GPU_MEMORY: &str = r"\GPU Process Memory(*)\Dedicated Usage";
/// Дескрипторы запроса и счётчика PDH — числа.
#[allow(non_camel_case_types)]
type PDH_HQUERY = isize;
#[allow(non_camel_case_types)]
type PDH_HCOUNTER = isize;
/// Буфера не хватило: так счётчик сообщает нужный размер.
const PDH_MORE_DATA: u32 = 0x8000_07D2;
/// Значение счётчика годно: свежее или прежнее.
const PDH_VALID: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Доля всех ядер машины в процентах, как в диспетчере задач.
    pub cpu: f32,
    /// Рабочий набор всех процессов, байты: то, что сейчас лежит в оперативной памяти.
    pub mem: u64,
    /// Загрузка самого занятого движка видеокарты, проценты.
    pub gpu: f32,
    /// Выделенная видеопамять, байты.
    pub vram: u64,
}

/// Одно показание счётчика видеокарты: процесс, движок, значение.
#[derive(Clone, Debug, PartialEq)]
struct GpuReading {
    pid: u32,
    engine: String,
    value: f64,
}

/// Замерщик помнит прошлое процессорное время групп и отдельных процессов: загрузка — это
/// разница двух замеров.
pub struct Sampler {
    cores: f64,
    seen: HashMap<String, u64>,
    seen_pids: HashMap<u32, u64>,
    at: Instant,
    gpu: Option<Gpu>,
}

impl Default for Sampler {
    fn default() -> Self {
        Sampler::new()
    }
}

impl Sampler {
    pub fn new() -> Sampler {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        Sampler { cores, seen: HashMap::new(), seen_pids: HashMap::new(), at: Instant::now(), gpu: Gpu::open() }
    }

    /// Расход каждой группы. Группа — первое поле пары; всё с одной группой складывается.
    /// `loose` — процессы без задания: серверы, запущенные мимо менеджера.
    /// У группы и процесса, которых замерщик видит впервые, загрузка процессора ещё нулевая.
    pub fn sample(&mut self, jobs: &[(String, Arc<Job>)], loose: &[(String, Vec<u32>)]) -> HashMap<String, Usage> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.at).as_nanos() as f64 / 100.0;
        self.at = now;

        let mut out: HashMap<String, Usage> = HashMap::new();
        let mut times: HashMap<String, u64> = HashMap::new();
        let mut owner: HashMap<u32, String> = HashMap::new();
        for (group, job) in jobs {
            *times.entry(group.clone()).or_default() += job.cpu_time();
            let usage = out.entry(group.clone()).or_default();
            for pid in job.pids() {
                usage.mem += working_set(pid);
                owner.insert(pid, group.clone());
            }
        }

        // Процессорное время за промежуток между замерами, по группам.
        let mut spent: HashMap<String, u64> = HashMap::new();
        for (group, &time) in &times {
            // Перезапуск обнуляет счётчик задания: разница уходит в минус и считается нулём.
            if let Some(&before) = self.seen.get(group) {
                *spent.entry(group.clone()).or_default() += time.saturating_sub(before);
            }
        }
        self.seen = times;

        let mut pid_times: HashMap<u32, u64> = HashMap::new();
        for (group, pids) in loose {
            let usage = out.entry(group.clone()).or_default();
            for &pid in pids {
                // Процесс задания уже посчитан вместе с заданием.
                if owner.contains_key(&pid) {
                    continue;
                }
                usage.mem += working_set(pid);
                owner.insert(pid, group.clone());
                let Some(time) = process_cpu_time(pid) else { continue };
                if let Some(&before) = self.seen_pids.get(&pid) {
                    *spent.entry(group.clone()).or_default() += time.saturating_sub(before);
                }
                pid_times.insert(pid, time);
            }
        }
        self.seen_pids = pid_times;

        if elapsed > 0.0 {
            for (group, spent) in spent {
                let share = spent as f64 / (elapsed * self.cores) * 100.0;
                out.entry(group).or_default().cpu = share.clamp(0.0, 100.0) as f32;
            }
        }

        if let Some(gpu) = &self.gpu {
            // Счётчики опрашиваются и без серверов: загрузка считается между двумя опросами,
            // и после долгого перерыва первое значение было бы средним за весь перерыв.
            let (engines, memory) = gpu.read();
            for (group, load) in busiest_engine(&engines, &owner) {
                out.entry(group).or_default().gpu = load;
            }
            for r in memory {
                if let Some(group) = owner.get(&r.pid) {
                    out.entry(group.clone()).or_default().vram += r.value.max(0.0) as u64;
                }
            }
        }
        out
    }
}

/// Загрузка видеокарты по группам: у каждого движка складываются процессы группы, берётся самый
/// занятый движок. Сложить движки нельзя: декодирование видео и 3D идут параллельно, и сумма
/// показала бы больше ста процентов на свободной карте.
fn busiest_engine(readings: &[GpuReading], owner: &HashMap<u32, String>) -> HashMap<String, f32> {
    let mut by_engine: HashMap<(&str, &str), f64> = HashMap::new();
    for r in readings {
        if let Some(group) = owner.get(&r.pid) {
            *by_engine.entry((group.as_str(), r.engine.as_str())).or_default() += r.value;
        }
    }
    let mut out: HashMap<String, f32> = HashMap::new();
    for ((group, _), load) in by_engine {
        let load = load.clamp(0.0, 100.0) as f32;
        let best = out.entry(group.to_string()).or_default();
        *best = best.max(load);
    }
    out
}

/// Имя экземпляра счётчика → номер процесса и остаток имени (видеокарта и движок).
fn parse_instance(name: &str) -> Option<(u32, &str)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, engine) = rest.split_once('_')?;
    Some((pid.parse().ok()?, engine))
}

/// Сколько оперативной памяти установлено, байты.
pub fn total_memory() -> u64 {
    // SAFETY: структура обнулена, её размер задан перед вызовом.
    unsafe {
        let mut status: MEMORYSTATUSEX = mem::zeroed();
        status.dwLength = mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 { 0 } else { status.ullTotalPhys }
    }
}

/// Процессорное время процесса с его запуска, в сотнях наносекунд.
fn process_cpu_time(pid: u32) -> Option<u64> {
    // SAFETY: дескриптор закрывается здесь же; структуры обнулены.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
            (mem::zeroed(), mem::zeroed(), mem::zeroed(), mem::zeroed());
        let ok = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(process);
        let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        (ok != 0).then(|| ticks(kernel) + ticks(user))
    }
}

/// Рабочий набор процесса. Чужой или уже завершившийся процесс даёт ноль.
fn working_set(pid: u32) -> u64 {
    // SAFETY: дескриптор закрывается здесь же, структура передана с собственным размером.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return 0;
        }
        let mut counters: PROCESS_MEMORY_COUNTERS = mem::zeroed();
        let size = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        let ok = K32GetProcessMemoryInfo(process, &mut counters, size);
        CloseHandle(process);
        if ok == 0 { 0 } else { counters.WorkingSetSize as u64 }
    }
}

/// Открытый запрос к счётчикам видеокарты. Счётчика может не быть (старая система, нет
/// драйвера): тогда соответствующее поле пустое, а показания нулевые.
struct Gpu {
    query: PDH_HQUERY,
    engine: Option<PDH_HCOUNTER>,
    memory: Option<PDH_HCOUNTER>,
}

impl Gpu {
    fn open() -> Option<Gpu> {
        // SAFETY: запрос закрывается в Drop; пути счётчиков — строки с нулём на конце.
        unsafe {
            let mut query: PDH_HQUERY = 0;
            if PdhOpenQueryW(ptr::null(), 0, &mut query) as u32 != 0 {
                return None;
            }
            let gpu = Gpu { query, engine: add_counter(query, GPU_ENGINE), memory: add_counter(query, GPU_MEMORY) };
            if gpu.engine.is_none() && gpu.memory.is_none() {
                return None;
            }
            // Загрузка считается между двумя опросами: первый делаем сразу.
            PdhCollectQueryData(gpu.query);
            Some(gpu)
        }
    }

    /// Свежие показания: загрузка движков и видеопамять по процессам.
    fn read(&self) -> (Vec<GpuReading>, Vec<GpuReading>) {
        // SAFETY: запрос и счётчики живы, пока жив `self`.
        unsafe {
            if PdhCollectQueryData(self.query) as u32 != 0 {
                return (Vec::new(), Vec::new());
            }
            let engines = self.engine.map(|c| values(c, true)).unwrap_or_default();
            let memory = self.memory.map(|c| values(c, false)).unwrap_or_default();
            (engines, memory)
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        // SAFETY: запрос открыт в `open` и больше нигде не закрывается.
        unsafe { PdhCloseQuery(self.query) };
    }
}

unsafe fn add_counter(query: PDH_HQUERY, path: &str) -> Option<PDH_HCOUNTER> {
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut counter: PDH_HCOUNTER = 0;
    // Английское имя счётчика работает при любом языке системы.
    (PdhAddEnglishCounterW(query, wide.as_ptr(), 0, &mut counter) as u32 == 0).then_some(counter)
}

/// Все экземпляры счётчика со значениями. `fraction` — дробное значение (проценты), иначе целое.
unsafe fn values(counter: PDH_HCOUNTER, fraction: bool) -> Vec<GpuReading> {
    let format = if fraction { PDH_FMT_DOUBLE } else { PDH_FMT_LARGE };
    let mut size: u32 = 0;
    let mut count: u32 = 0;
    let code = PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, ptr::null_mut());
    if code as u32 != PDH_MORE_DATA || size == 0 {
        return Vec::new();
    }
    // Буфер из u64 даёт выравнивание структуры; имена экземпляров лежат в нём же, за структурами.
    let mut buf = vec![0u64; size as usize / 8 + 1];
    size = (buf.len() * 8) as u32;
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, items) as u32 != 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for item in std::slice::from_raw_parts(items, count as usize) {
        if item.FmtValue.CStatus > PDH_VALID || item.szName.is_null() {
            continue;
        }
        let len = (0..).take_while(|&i| *item.szName.add(i) != 0).count();
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len));
        let Some((pid, engine)) = parse_instance(&name) else { continue };
        let value = if fraction {
            item.FmtValue.Anonymous.doubleValue
        } else {
            item.FmtValue.Anonymous.largeValue as f64
        };
        out.push(GpuReading { pid, engine: engine.to_string(), value });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::windows::io::AsRawHandle, process::Command, thread, time::Duration};

    fn reading(pid: u32, engine: &str, value: f64) -> GpuReading {
        GpuReading { pid, engine: engine.into(), value }
    }

    #[test]
    fn instance_name_gives_pid_and_engine() {
        assert_eq!(
            parse_instance("pid_1234_luid_0x00000000_0x0000D3A5_phys_0_eng_3_engtype_3D"),
            Some((1234, "luid_0x00000000_0x0000D3A5_phys_0_eng_3_engtype_3D"))
        );
        assert_eq!(parse_instance("pid_77_luid_0x0_0x1_phys_0"), Some((77, "luid_0x0_0x1_phys_0")));
        assert_eq!(parse_instance("_Total"), None);
        assert_eq!(parse_instance("pid_x_luid"), None);
    }

    #[test]
    fn gpu_load_is_the_busiest_engine_of_the_group() {
        let owner: HashMap<u32, String> =
            [(1, "a".to_string()), (2, "a".to_string()), (3, "b".to_string())].into_iter().collect();
        let readings = [
            // Два процесса группы на одном движке складываются.
            reading(1, "eng_0_engtype_3D", 20.0),
            reading(2, "eng_0_engtype_3D", 15.0),
            // Другой движок той же группы не прибавляется, а сравнивается.
            reading(1, "eng_5_engtype_VideoDecode", 30.0),
            reading(3, "eng_0_engtype_3D", 70.0),
            reading(3, "eng_1_engtype_Cuda", 80.0),
            // Чужой процесс в счёт не идёт.
            reading(99, "eng_0_engtype_3D", 90.0),
        ];
        let load = busiest_engine(&readings, &owner);
        assert_eq!(load["a"], 35.0);
        assert_eq!(load["b"], 80.0);
        assert_eq!(load.len(), 2);
    }

    #[test]
    fn total_memory_is_known() {
        assert!(total_memory() > 1 << 30);
    }

    #[test]
    fn busy_process_shows_cpu_and_memory() {
        let job = Arc::new(Job::new().unwrap());
        // Пустой бесконечный цикл занимает одно ядро целиком.
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "while ($true) {}"])
            .spawn()
            .unwrap();
        job.assign(child.as_raw_handle() as _).unwrap();

        let jobs = vec![("repo".to_string(), job.clone())];
        let mut sampler = Sampler::new();
        // PowerShell сначала загружается: даём ему дойти до цикла.
        thread::sleep(Duration::from_millis(1500));
        let first = sampler.sample(&jobs, &[]);
        assert_eq!(first["repo"].cpu, 0.0, "первый замер без прошлого даёт ноль");
        assert!(first["repo"].mem > 0);

        thread::sleep(Duration::from_millis(1500));
        let second = sampler.sample(&jobs, &[]);
        // Порог ниже целого ядра: под песочницей или при нагрузке цикл получает лишь его часть.
        let of_one_core = second["repo"].cpu * sampler.cores as f32;
        assert!(of_one_core > 2.0, "{:?} при {} ядрах", second["repo"], sampler.cores);
        assert!(second["repo"].cpu <= 100.0);

        job.terminate().unwrap();
        let _ = child.wait();
        thread::sleep(Duration::from_millis(100));
        let after = sampler.sample(&jobs, &[]);
        assert_eq!(after["repo"].mem, 0, "у завершённого дерева памяти нет");
    }

    #[test]
    fn group_without_jobs_is_absent() {
        assert!(Sampler::new().sample(&[], &[]).is_empty());
    }

    /// Процессы без задания (сервер, запущенный мимо менеджера) меряются по каждому процессу.
    #[test]
    fn loose_processes_are_measured() {
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-Command", "while ($true) {}"])
            .spawn()
            .unwrap();
        let loose = vec![("repo".to_string(), vec![child.id()])];
        let mut sampler = Sampler::new();
        thread::sleep(Duration::from_millis(1500));
        let first = sampler.sample(&[], &loose);
        assert_eq!(first["repo"].cpu, 0.0, "первый замер без прошлого даёт ноль");
        assert!(first["repo"].mem > 0);

        thread::sleep(Duration::from_millis(1500));
        let second = sampler.sample(&[], &loose);
        let of_one_core = second["repo"].cpu * sampler.cores as f32;
        let _ = child.kill();
        let _ = child.wait();
        assert!(of_one_core > 2.0, "{:?} при {} ядрах", second["repo"], sampler.cores);
    }

    /// Показания счётчиков видеокарты на этой машине: `cargo test -p lsm-core gpu_dump -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn gpu_dump() {
        let gpu = Gpu::open().expect("счётчиков видеокарты нет");
        thread::sleep(Duration::from_secs(1));
        let (engines, memory) = gpu.read();
        println!("движков с показаниями: {}, процессов с видеопамятью: {}", engines.len(), memory.len());
        let mut busy: Vec<&GpuReading> = engines.iter().filter(|r| r.value > 0.0).collect();
        busy.sort_by(|a, b| b.value.total_cmp(&a.value));
        for r in busy.iter().take(10) {
            println!("  {:>6}  {:>6.1}%  {}", r.pid, r.value, r.engine);
        }
        let mut top: Vec<&GpuReading> = memory.iter().collect();
        top.sort_by(|a, b| b.value.total_cmp(&a.value));
        for r in top.iter().take(10) {
            println!("  {:>6}  {:>7.0} МБ  {}", r.pid, r.value / 1048576.0, r.engine);
        }
    }
}
