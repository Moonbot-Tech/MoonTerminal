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

/// Полный тикер для подписи на чарте: `BTCUSDT` → `BTC-USDT`. HIP-3 (`xyz:BIRD`) → `BIRD`
/// (dex-префикс и неявный USDC не показываем). Если quote не распознан — монета без префикса.
pub fn display_pair(market: &str) -> String {
    let after_dex = strip_dex(market);
    let quote = resolve_quote(after_dex);
    if quote.is_empty() {
        return coin_of_market(market).to_string();
    }
    format!("{}-{}", base_symbol(after_dex, &quote), quote)
}

/// HIP-3-рынок Hyperliquid: имя несёт dex-префикс `dex_name:coin` (`xyz:BIRD`). Двоеточие в
/// имени рынка бывает ТОЛЬКО у HIP-3 (обычные биржи шлют пустой `dex_name`, без `:`).
pub fn is_hip3(market: &str) -> bool {
    market.contains(':')
}

/// Имя dex у HIP-3-рынка (`xyz:BIRD` → `Some("xyz")`), иначе `None`.
pub fn dex_of_market(market: &str) -> Option<&str> {
    market.split_once(':').map(|(dex, _)| dex)
}

/// Часть имени рынка ПОСЛЕ dex-префикса: `xyz:BIRD` → `BIRD`, `ADAUSDT` → `ADAUSDT`.
fn strip_dex(market: &str) -> &str {
    market.rsplit(':').next().unwrap_or(market)
}

/// КАНОНИЧЕСКАЯ монета рынка (для ЧС/матчинга/показа тикера): срезаем dex-префикс, затем
/// quote-суффикс. `xyz:BIRD` → `BIRD`, `BIRDUSDC` → `BIRD`, `ADAUSDT` → `ADA`.
///
/// ВАЖНО: это НЕ ключ рынка. Для подписки/открытия/поиска всегда берём полное имя рынка
/// (`xyz:BIRD`) — сервер резолвит по `market_name`. `coin_of_market` — только «как называется
/// монета» (сервер сравнивает ЧС именно с этим `market_currency`, без префикса).
pub fn coin_of_market(market: &str) -> &str {
    let after_dex = strip_dex(market);
    base_symbol(after_dex, &resolve_quote(after_dex))
}

#[cfg(test)]
mod tests {
    use super::coin_of_market;

    #[test]
    fn coin_plain() {
        assert_eq!(coin_of_market("ADAUSDT"), "ADA");
        assert_eq!(coin_of_market("BTCUSDT"), "BTC");
        assert_eq!(coin_of_market("VANRY_USDT"), "VANRY");
    }

    #[test]
    fn coin_hip3_strips_dex() {
        assert_eq!(coin_of_market("xyz:BIRD"), "BIRD");
        assert_eq!(coin_of_market("xyz:BIRDUSDC"), "BIRD");
    }
}
