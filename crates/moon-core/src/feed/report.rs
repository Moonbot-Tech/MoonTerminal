//! Report-сторона feed: полные данные ордеров из живой модели (по uid/db_id)
//! и сборка `ReportRow` из close-SQL + снимка для SQLite-writer'а.

use std::collections::HashMap;

use crate::config::ServerConfig;
use crate::db::{ReportRow, ReportTx};

/// Полный снимок полей ордера из живой модели, запоминаемый по серверному db_id.
/// Источник всех данных, которых нет в close-SQL (монета/открытие/цены/статы).
pub(super) struct OrderMeta {
    pub coin: String,
    pub isshort: bool,
    pub buyprice: f64,
    pub sellprice: f64,
    pub quantity: f64,
    pub spentbtc: f64,
    pub gainedbtc: f64,
    pub lev: i64,
    pub strategyid: i64,
    pub taskid: i64,
    pub exorderid: Option<String>,
    pub emulator: bool,
    pub buydate: Option<i64>,
    pub sellsetdate: Option<i64>,
    pub closedate: Option<i64>,
}

/// Индекс полных данных ордеров. Копим по СТАБИЛЬНОМУ uid (есть с открытия), а
/// db_id у открытого ордера почти всегда 0 — он присваивается лишь перед
/// закрытием, когда строка пишется в Orders DB ядра. Поэтому держим ещё карту
/// db_id→uid (заполняется в момент появления db_id). На close-report (там только
/// db_id) идём db_id → uid → полные данные.
#[derive(Default)]
pub(super) struct OrderIndex {
    by_uid: HashMap<u64, OrderMeta>,
    dbid_to_uid: HashMap<i32, u64>,
}

impl OrderIndex {
    pub fn remember(&mut self, uid: u64, meta: OrderMeta) {
        self.by_uid.insert(uid, meta);
    }

    pub fn map_dbid(&mut self, db_id: i32, uid: u64) {
        self.dbid_to_uid.insert(db_id, uid);
    }

    pub fn by_uid(&self, uid: u64) -> Option<&OrderMeta> {
        self.by_uid.get(&uid)
    }

    pub fn by_dbid(&self, db_id: i32) -> Option<&OrderMeta> {
        self.dbid_to_uid
            .get(&db_id)
            .and_then(|uid| self.by_uid.get(uid))
    }
}

/// Разбирает close-SQL (insert ИЛИ update) и шлёт `ReportRow` SQLite-writer'у.
/// Поля из SQL финальные/авторитетные; чего там нет — из снимка модели `m`.
pub(super) fn send_close_report(
    tx_db: &ReportTx,
    server: &ServerConfig,
    db_id: i64,
    sql: String,
    m: Option<&OrderMeta>,
) {
    let p = crate::db::parse_report_sql(&sql);
    // Логируем СЫРОЙ report-SQL в logs/commands.log — чтобы видеть, INSERT или
    // UPDATE шлёт ядро и что внутри.
    let form = if sql
        .trim_start()
        .get(..6)
        .map(|s| s.eq_ignore_ascii_case("insert"))
        .unwrap_or(false)
    {
        "INSERT"
    } else {
        "UPDATE"
    };
    crate::applog::command(&format!(
        "core={} ({}) db_id={} form={} link={} coin={:?} buydate={:?}\n    SQL: {}",
        server.uid,
        server.name,
        db_id,
        form,
        m.is_some(),
        m.map(|x| x.coin.clone()).or_else(|| p.coin.clone()),
        m.and_then(|x| x.buydate).or(p.buydate),
        sql,
    ));
    let _ = tx_db.send(ReportRow {
        core_uid: server.uid, // СТАБИЛЬНЫЙ uid, не рантайм-id
        core_name: server.name.clone(),
        db_id,
        taskid: p.taskid.or_else(|| m.map(|m| m.taskid)),
        exorderid: m.and_then(|m| m.exorderid.clone()),
        coin: p.coin.or_else(|| m.map(|m| m.coin.clone())),
        isshort: p.isshort.or_else(|| m.map(|m| m.isshort)),
        buydate: p.buydate.or_else(|| m.and_then(|m| m.buydate)),
        sellsetdate: p.sellsetdate.or_else(|| m.and_then(|m| m.sellsetdate)),
        closedate: p.close_date.or_else(|| m.and_then(|m| m.closedate)),
        quantity: p.quantity.or_else(|| m.map(|m| m.quantity)),
        buyprice: p.buyprice.or_else(|| m.map(|m| m.buyprice)),
        sellprice: p.sellprice.or_else(|| m.map(|m| m.sellprice)),
        spentbtc: p.spent_btc.or_else(|| m.map(|m| m.spentbtc)),
        gainedbtc: p.gained_btc.or_else(|| m.map(|m| m.gainedbtc)),
        profitbtc: p.profit_btc.or_else(|| m.map(|m| m.gainedbtc - m.spentbtc)),
        lev: p.lev.or_else(|| m.map(|m| m.lev)),
        strategyid: p.strategyid.or_else(|| m.map(|m| m.strategyid)),
        emulator: m.map(|m| m.emulator),
        status: p.status,
        sellreason: p.sell_reason,
        comment: p.comment,
        extras: p.all,
        sql,
    });
}
