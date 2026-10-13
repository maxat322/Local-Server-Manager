//! Пауза сервера: заморозка всех процессов его задания.
//!
//! Замороженный процесс не получает процессорного времени, но остаётся в памяти со всем, что
//! успел собрать, и продолжает ровно с того же места. Заодно его физическая память вытесняется
//! в файл подкачки: без этого пауза экономила бы только процессор.

use crate::job::Job;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_SUSPEND_RESUME},
};

// Заморозки процесса целиком в документированном API нет, только по одному потоку. Эти две
// функции ядра делают это атомарно для всех потоков; ими же пользуются Process Explorer
// и диспетчер задач.
#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(process: HANDLE) -> i32;
    fn NtResumeProcess(process: HANDLE) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn SetProcessWorkingSetSize(process: HANDLE, min: usize, max: usize) -> i32;
}

/// Пока одни процессы замораживаются, другие успевают породить потомков. Список задания
/// перечитывается, пока в нём появляются новые номера, но не больше этого числа раз.
const ROUNDS: usize = 6;

fn with_process<T>(pid: u32, access: u32, action: impl FnOnce(HANDLE) -> T) -> Option<T> {
    // SAFETY: дескриптор закрывается здесь же.
    unsafe {
        let handle = OpenProcess(access, 0, pid);
        if handle.is_null() {
            return None;
        }
        let result = action(handle);
        CloseHandle(handle);
        Some(result)
    }
}

/// Замораживает все процессы задания и возвращает номера тех, кого удалось остановить.
/// Именно их потом нужно разморозить: у заморозки есть счётчик, лишний вызов оставил бы
/// процесс остановленным.
pub fn suspend(job: &Job) -> Vec<u32> {
    let mut frozen: Vec<u32> = Vec::new();
    let mut tried: Vec<u32> = Vec::new();
    for _ in 0..ROUNDS {
        let fresh: Vec<u32> = job.pids().into_iter().filter(|pid| !tried.contains(pid)).collect();
        if fresh.is_empty() {
            break;
        }
        for pid in fresh {
            tried.push(pid);
            // SAFETY: дескриптор жив на время вызова.
            let ok = with_process(pid, PROCESS_SUSPEND_RESUME, |h| unsafe { NtSuspendProcess(h) } >= 0);
            if ok == Some(true) {
                frozen.push(pid);
            }
        }
    }
    for &pid in &frozen {
        // Пара (-1, -1) просит систему убрать из физической памяти всё, что можно.
        // SAFETY: дескриптор жив на время вызова.
        with_process(pid, PROCESS_SET_QUOTA, |h| unsafe { SetProcessWorkingSetSize(h, usize::MAX, usize::MAX) });
    }
    frozen
}

/// Размораживает процессы, остановленные [`suspend`]. Тех, кого уже нет, молча пропускает.
pub fn resume(pids: &[u32]) {
    for &pid in pids {
        // SAFETY: дескриптор жив на время вызова.
        with_process(pid, PROCESS_SUSPEND_RESUME, |h| unsafe { NtResumeProcess(h) });
    }
}
