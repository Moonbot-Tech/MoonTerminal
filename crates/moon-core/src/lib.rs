//! MoonTerminal backend core — UI-агностичное ядро терминала.
//!
//! Здесь живёт всё, что не зависит от движка отображения (GPUI/DX11): поток данных
//! от ядра MoonBot, мульти-ядра/дедуп маркет-данных, конфиг с секретами, локальная
//! БД отчётов и доменные типы. Общение с UI — через `feed::FeedMsg` (ядро → UI) и
//! `feed::CoreCmd` (UI → ядро); UI никогда не зовёт транспорт (moonproto) напрямую.
//!
//! Используется GPUI-оболочкой `moonterminal` (зависит от core, не наоборот). Старая
//! egui-оболочка `moon-terminal` удалена.

pub mod applog;
pub mod config;
pub mod data;
pub mod db;
pub mod detect_diag;
pub mod feed;
pub mod market;
pub mod metrics;
pub mod palette;
pub mod session;
pub mod symbol;
pub mod util;
