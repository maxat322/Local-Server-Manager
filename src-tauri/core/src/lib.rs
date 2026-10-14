//! Ядро Local Server Manager: настройки, поиск способов запуска и управление процессами.
//!
//! Об окне и трее ядро ничего не знает и от Tauri не зависит. Поэтому весь жизненный цикл
//! сервера — запуск, остановка дерева процессов, определение порта — проверяется тестами
//! на настоящих процессах, без сборки оболочки.

pub mod badge;
pub mod config;
pub mod foreign;
pub mod freeze;
pub mod job;
pub mod portbook;
pub mod ports;
pub mod pty;
pub mod runner;
pub mod scan;
pub mod text;
pub mod usage;
