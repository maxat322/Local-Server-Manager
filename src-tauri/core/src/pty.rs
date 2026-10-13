//! Псевдоконсоль Windows (ConPTY): сервер запускается в настоящем терминале, только без окна.
//!
//! Через обычные каналы скрипты работают плохо: `pause` и `timeout` не видят клавиатуры,
//! инструменты отключают цвета и индикаторы прогресса, а Ctrl+C отправить некуда.
//! С псевдоконсолью процесс уверен, что запущен в терминале: пишет в него как обычно, читает ввод,
//! а приложение получает поток вывода и передаёт нажатия клавиш.

use crate::job::Job;
use std::{
    ffi::{c_void, OsStr},
    fs::File,
    io::{self, Write},
    mem,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    ptr,
    sync::Mutex,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON},
        Pipes::CreatePipe,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
            ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

/// Псевдоконсоль и канал ввода в неё.
pub struct Pty {
    /// Дескриптор консоли числом: так структура остаётся потокобезопасной при любом
    /// представлении HPCON. `None` после закрытия.
    console: Mutex<Option<usize>>,
    input: Mutex<Option<File>>,
}

impl Pty {
    /// Передаёт процессу ввод: текст, Enter, Ctrl+C (байт 0x03).
    pub fn write(&self, data: &[u8]) -> io::Result<()> {
        match self.input.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Some(pipe) => pipe.write_all(data),
            None => Ok(()),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        if let Some(console) = *self.console.lock().unwrap_or_else(|e| e.into_inner()) {
            // SAFETY: дескриптор жив, пока лежит в `console`.
            unsafe { ResizePseudoConsole(console as HPCON, coord(cols, rows)) };
        }
    }

    /// Закрывает консоль. После этого поток вывода дочитывается до конца и закрывается:
    /// пока консоль открыта, конца у него нет, даже если все процессы уже завершились.
    pub fn close(&self) {
        self.input.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(console) = self.console.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: дескриптор изъят из `console`, второй раз закрыт не будет.
            unsafe { ClosePseudoConsole(console as HPCON) };
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.close();
    }
}

/// Корневой процесс сервера.
pub struct Process {
    handle: HANDLE,
    pub pid: u32,
}

// SAFETY: дескриптор процесса — число, функции ядра над ним потокобезопасны.
unsafe impl Send for Process {}

impl Process {
    /// Ждёт завершения и возвращает код выхода.
    pub fn wait(&self) -> i32 {
        let mut code: u32 = 0;
        // SAFETY: дескриптор жив, пока жива структура.
        unsafe {
            WaitForSingleObject(self.handle, INFINITE);
            GetExitCodeProcess(self.handle, &mut code);
        }
        code as i32
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: дескриптор получен от CreateProcessW и больше нигде не закрывается.
        unsafe { CloseHandle(self.handle) };
    }
}

pub struct Spawned {
    pub pty: Pty,
    /// Поток вывода консоли в UTF-8 с управляющими последовательностями терминала.
    pub output: File,
    pub process: Process,
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: cols.clamp(20, 500) as i16, Y: rows.clamp(5, 200) as i16 }
}

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(Some(0)).collect()
}

/// Запускает командную строку в новой псевдоконсоли и сразу помещает процесс в задание.
///
/// Процесс создаётся приостановленным и возобновляется только после привязки к заданию:
/// иначе шустрый `cmd.exe` успел бы породить потомка вне задания, и «Остановить» его бы не достал.
pub fn spawn(command_line: &str, cwd: &str, cols: u16, rows: u16, job: &Job) -> io::Result<Spawned> {
    // SAFETY: каждый дескриптор либо передаётся во владение структуре с Drop, либо закрывается
    // здесь же на всех путях выхода; размеры буферов берутся из ответов самих функций.
    unsafe {
        let (mut in_read, mut in_write): (HANDLE, HANDLE) = (ptr::null_mut(), ptr::null_mut());
        let (mut out_read, mut out_write): (HANDLE, HANDLE) = (ptr::null_mut(), ptr::null_mut());
        if CreatePipe(&mut in_read, &mut in_write, ptr::null(), 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        if CreatePipe(&mut out_read, &mut out_write, ptr::null(), 0) == 0 {
            let err = io::Error::last_os_error();
            CloseHandle(in_read);
            CloseHandle(in_write);
            return Err(err);
        }
        // С этого места наши концы каналов закроются сами при любом выходе.
        let input = File::from_raw_handle(in_write as _);
        let output = File::from_raw_handle(out_read as _);

        let mut console: HPCON = mem::zeroed();
        let result = CreatePseudoConsole(coord(cols, rows), in_read, out_write, 0, &mut console);
        // Консоль продублировала свои концы каналов, наши копии больше не нужны.
        CloseHandle(in_read);
        CloseHandle(out_write);
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        let pty = Pty { console: Mutex::new(Some(console as usize)), input: Mutex::new(Some(input)) };

        // Список атрибутов с единственной записью: «запусти в этой псевдоконсоли».
        let mut size: usize = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        let mut storage = vec![0usize; size / mem::size_of::<usize>() + 1];
        let attributes = storage.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let attached = UpdateProcThreadAttribute(
            attributes,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            console as *const c_void,
            mem::size_of::<HPCON>(),
            ptr::null_mut(),
            ptr::null(),
        );
        if attached == 0 {
            let err = io::Error::last_os_error();
            DeleteProcThreadAttributeList(attributes);
            return Err(err);
        }

        let mut startup: STARTUPINFOEXW = mem::zeroed();
        startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes;
        // Явно недействительные стандартные дескрипторы: иначе потомок может унаследовать
        // перенаправленный вывод родителя и писать мимо псевдоконсоли.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

        let mut command = wide(command_line);
        let directory = wide(cwd);
        let mut info: PROCESS_INFORMATION = mem::zeroed();
        let created = CreateProcessW(
            ptr::null(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            ptr::null(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        );
        let create_error = io::Error::last_os_error();
        DeleteProcThreadAttributeList(attributes);
        if created == 0 {
            return Err(create_error);
        }
        let process = Process { handle: info.hProcess, pid: info.dwProcessId };

        if let Err(err) = job.assign(info.hProcess) {
            TerminateProcess(info.hProcess, 1);
            CloseHandle(info.hThread);
            return Err(err);
        }
        ResumeThread(info.hThread);
        CloseHandle(info.hThread);

        Ok(Spawned { pty, output, process })
    }
}
