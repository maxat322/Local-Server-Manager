//! Job Object: способ держать в руках всё дерево процессов сервера.
//!
//! Дев-сервер почти никогда не один процесс: `start-local.cmd` поднимает `node`, тот —
//! `concurrently`, а он ещё несколько `node`. Убить корневой `cmd.exe` значит оставить всех
//! потомков работать и держать порты. Задание Windows помнит всех, кто в нём родился, даже если
//! их родитель уже вышел, и завершает их одной командой.

use std::{ffi::c_void, io, mem, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

/// Сколько идентификаторов процессов помещается в буфер одного запроса.
/// У самого развесистого дев-сервера их десятки, запас в два порядка.
const MAX_PIDS: usize = 2048;

pub struct Job(HANDLE);

// SAFETY: дескриптор задания — число, а функции ядра над ним потокобезопасны.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    /// Задание с `KILL_ON_JOB_CLOSE`: когда закрывается последний дескриптор (в том числе при
    /// аварийном завершении приложения), система сама убивает всех участников.
    pub fn new() -> io::Result<Job> {
        // SAFETY: структура обнулена и передана с собственным размером.
        unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }

    /// Помещает процесс в задание. Делать это нужно, пока процесс ещё приостановлен:
    /// всё, что он породит после, окажется в задании автоматически.
    pub fn assign(&self, process: HANDLE) -> io::Result<()> {
        // SAFETY: вызывающий передаёт живой дескриптор процесса.
        let ok = unsafe { AssignProcessToJobObject(self.0, process) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Завершает все процессы задания разом.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: дескриптор задания принадлежит нам.
        let ok = unsafe { TerminateJobObject(self.0, 1) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Идентификаторы живых процессов задания.
    pub fn pids(&self) -> Vec<u32> {
        // Буфер из u64 даёт выравнивание, которого требует структура со списком usize.
        let mut buf = vec![0u64; 1 + MAX_PIDS];
        // SAFETY: размер буфера передан в байтах; читаем не больше, чем система записала.
        unsafe {
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicProcessIdList,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * mem::size_of::<u64>()) as u32,
                ptr::null_mut(),
            );
            let list = buf.as_ptr() as *const JOBOBJECT_BASIC_PROCESS_ID_LIST;
            let count = (*list).NumberOfProcessIdsInList as usize;
            // При переполнении вызов возвращает ошибку, но заполненная часть списка верна.
            if ok == 0 && count == 0 {
                return Vec::new();
            }
            let first = ptr::addr_of!((*list).ProcessIdList) as *const usize;
            std::slice::from_raw_parts(first, count.min(MAX_PIDS)).iter().map(|&pid| pid as u32).collect()
        }
    }
}

impl Job {
    /// Процессорное время всего дерева с момента запуска, в сотнях наносекунд.
    /// Задание помнит и время уже завершившихся процессов: короткие потомки не теряются.
    pub fn cpu_time(&self) -> u64 {
        // SAFETY: структура обнулена и передана с собственным размером.
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = mem::zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut c_void,
                mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                ptr::null_mut(),
            );
            if ok == 0 { 0 } else { (info.TotalUserTime + info.TotalKernelTime).max(0) as u64 }
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: дескриптор создан в `new` и больше нигде не закрывается.
        unsafe { CloseHandle(self.0) };
    }
}
