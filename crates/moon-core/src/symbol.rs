//! Утилиты для отображения символа рынка. Ядро подключается к одному quote
//! (USDT/USDC/…), и в UI монету показываем БЕЗ этого суффикса: `ADAUSDT` → `ADA`.

/// Известные quote-валюты, по которым режем суффикс. Порядок — по длине (сначала
/// длинные), чтобы `FDUSD`/`USDC` срабатывали раньше `USD`.
const QUOTES: [&str; 9] = [
    "FDUSD", "TUSD", "USDC", "BUSD", "USDT", "USD", "BTC", "ETH", "BNB",
];

/// Quote подключения ядра, выведенный из его рынка по умолчанию (`server.market`).
/// `BTCUSDT` → `USDT`; если не распознан — пустая строка (тогда ничего не режем).
pub fn resolve_quote(market: &str) -> String {
    let up = market.to_ascii_uppercase();
    QUOTES
        .iter()
        .find(|q| up.ends_with(*q) && up.len() > q.len())
        .map(|q| q.to_string())
        .unwrap_or_default()
}

/// Является ли валюта USD-стейблом (курс к USD ≈ 1). Список зеркалит `feed::assets`.
pub fn is_usd_stable(currency: &str) -> bool {
    matches!(
        currency.to_ascii_uppercase().as_str(),
        "USDT" | "USDC" | "BUSD" | "USD" | "FDUSD" | "TUSD" | "DAI" | "USDP"
    )
}

/// Базовая монета: срезает `quote` с конца `sym` (если совпал). `quote` пуст или
/// не подошёл → возвращаем символ как есть.
///
/// Gate и подобные биржи разделяют символ подчёркиванием (`VANRY_USDT`, `1INCH_USDT`):
/// после среза `USDT` остаётся хвостовой разделитель `VANRY_` — убираем его (`_`/`-`/`/`),
/// иначе в таблицах токен показывается как «VANRY_».
pub fn base_symbol<'a>(sym: &'a str, quote: &str) -> &'a str {
    if !quote.is_empty() && sym.len() > quote.len() && sym.to_ascii_uppercase().ends_with(quote) {
        let base = &sym[..sym.len() - quote.len()];
        base.trim_end_matches(|c| c == '_' || c == '-' || c == '/')
    } else {
        sym
    }
}

/// Полный тикер для подписи на чарте: `BTCUSDT` → `BTC-USDT`. Если quote не распознан —
/// возвращаем рынок как есть (без дефиса).
pub fn display_pair(market: &str) -> String {
    let quote = resolve_quote(market);
    if quote.is_empty() {
        return market.to_string();
    }
    format!("{}-{}", base_symbol(market, &quote), quote)
}
