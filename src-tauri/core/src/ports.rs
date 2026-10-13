//! Какие TCP-порты сейчас слушаются и каким процессом.
//!
//! По этому приложение понимает, что сервер действительно поднялся, а не просто запущен процесс,
//! и на каком адресе его открывать. Спрашиваем таблицу у системы напрямую: разбирать вывод
//! `netstat` раз в секунду и дорого, и зависит от языка системы.

use std::{collections::HashMap, ffi::c_void, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    },
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    },
};

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// Пары «процесс, порт» для всех слушающих сокетов. Node на новых версиях привязывает
/// `localhost` к `::1`, поэтому таблица IPv6 нужна наравне с IPv4.
pub fn listening() -> Vec<(u32, u16)> {
    let mut out = Vec::new();

    if let Some(buf) = table(AF_INET) {
        // SAFETY: система заполнила буфер таблицей заявленного размера, читаем `dwNumEntries` строк.
        unsafe {
            let t = buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
            let rows = ptr::addr_of!((*t).table) as *const MIB_TCPROW_OWNER_PID;
            for row in std::slice::from_raw_parts(rows, (*t).dwNumEntries as usize) {
                out.push((row.dwOwningPid, port(row.dwLocalPort)));
            }
        }
    }
    if let Some(buf) = table(AF_INET6) {
        // SAFETY: то же для таблицы IPv6.
        unsafe {
            let t = buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID;
            let rows = ptr::addr_of!((*t).table) as *const MIB_TCP6ROW_OWNER_PID;
            for row in std::slice::from_raw_parts(rows, (*t).dwNumEntries as usize) {
                out.push((row.dwOwningPid, port(row.dwLocalPort)));
            }
        }
    }
    out
}

/// Имена программ всех процессов системы: номер процесса → `node.exe`.
///
/// Берётся снимком системы, а не открытием каждого процесса: службы под чужими учётными
/// записями (тот же PostgreSQL) открыть нельзя, а в снимке их имена есть.
pub fn process_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    // SAFETY: снимок закрывается здесь же, размер структуры задан перед первым вызовом.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return names;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            names.insert(entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len]));
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    // У ядра в снимке имя «[System Process]» и «System»: приводим к одному понятному.
    names.insert(0, "System".into());
    names
}

/// Порт лежит в младших двух байтах в сетевом порядке.
fn port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

/// Таблица слушающих сокетов одного семейства адресов. Буфер из u32 даёт нужное выравнивание.
fn table(family: u32) -> Option<Vec<u32>> {
    let mut size: u32 = 0;
    // SAFETY: первый вызов с пустым буфером только сообщает размер; второй пишет не больше `size`.
    unsafe {
        GetExtendedTcpTable(ptr::null_mut(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0);
        // Между двумя вызовами таблица может вырасти: повторяем, пока буфера не хватит.
        for _ in 0..4 {
            let mut buf = vec![0u32; size as usize / 4 + 16];
            size = (buf.len() * 4) as u32;
            let code = GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut c_void,
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            match code {
                0 => return Some(buf),
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn own_listening_socket_is_visible() {
        let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
        let v6 = TcpListener::bind("[::1]:0").unwrap();
        let me = std::process::id();
        let table = listening();
        assert!(table.contains(&(me, v4.local_addr().unwrap().port())));
        assert!(table.contains(&(me, v6.local_addr().unwrap().port())));
    }

    #[test]
    fn own_process_has_a_name() {
        let names = process_names();
        let name = &names[&std::process::id()];
        assert!(name.to_lowercase().ends_with(".exe"), "{name}");
        assert_eq!(names[&4], "System");
    }
}
