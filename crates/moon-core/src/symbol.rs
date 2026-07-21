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

/// The token a strategy's coin list (`CoinsBlackList` / `CoinsWhiteList`) uses for
/// a coin, derived from the coin name as the REPORT writes it.
///
/// The report names a coin together with its contract (`BTC_RP` perpetual,
/// `LTC_1230` quarterly), while the strategy lists hold bare tokens (`ANC, GALA`,
/// `mith,tribe`). Without stripping the tail no futures coin would ever match its
/// own list. Case is left alone — the comparison is case-insensitive anyway, and
/// the report legitimately holds `kFLOKI` / `10kSATS`.
///
/// Only KNOWN tails are stripped (`_RP` and a 4-digit `_MMDD`), so a coin genuinely
/// named `FOO_BAR` — or `FOO_2` — stays itself instead of collapsing to `FOO`.
pub fn strip_contract_suffix(coin: &str) -> &str {
    let Some((base, suffix)) = coin.rsplit_once('_') else {
        return coin;
    };
    if base.is_empty() {
        return coin;
    }
    let known = suffix.eq_ignore_ascii_case("RP")
        || (suffix.len() == 4 && suffix.bytes().all(|b| b.is_ascii_digit()));
    if known {
        base
    } else {
        coin
    }
}

/// THE comparison key for "is this coin the one that list names?".
///
/// Both sides of a strategy coin list go through this: the report's name (which carries a
/// contract) and the list's own token (bare, in whatever case the user typed). Callers must
/// not hand-roll the comparison — two spellings of the rule is how one screen starts
/// disagreeing with another about whether a coin is blacklisted.
pub fn coin_match_key(coin: &str) -> String {
    strip_contract_suffix(coin.trim()).to_ascii_uppercase()
}

/// A strategy coin list (`"ANC, GALA"` / `"mith,tribe"`) as DISTINCT match keys.
///
/// The separator set and the key both live here so the terminal's coin table, its
/// counters and the per-strategy column cannot each decide for themselves what a list
/// contains — they already have to agree on the number they print.
pub fn parse_coin_list(text: &str) -> std::collections::HashSet<String> {
    split_coin_list(text).map(coin_match_key).collect()
}

/// The list's entries AS WRITTEN — same splitting as [`parse_coin_list`], without the fold.
///
/// Matching a coin needs the folded token; REPRODUCING the field needs the original text.
/// `BTC, BTC_0626, BTC_0925` are three entries that all match the coin `BTC`, so anything
/// that presents the folded form as the field's value silently drops two of them.
pub fn split_coin_list(text: &str) -> impl Iterator<Item = &str> {
    // Brackets and quotes count as separators too: the field is plain text in the strategy,
    // but a core that ever spells it as a JSON array would otherwise yield tokens like
    // `["BTC` that match no coin and inflate every count built on this.
    text.split([',', ';', ' ', '\n', '\r', '\t', '[', ']', '"', '\''])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
