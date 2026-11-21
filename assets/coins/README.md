# Coin icons

32×32 color PNG icons from [spothq/cryptocurrency-icons](https://github.com/spothq/cryptocurrency-icons)
(CC0 1.0 Universal — public domain). File name = lowercase coin symbol (`btc.png`).

`crates/moon-ui-gpui/src/media/coin_icons.rs` embeds this set and draws it in the Assets
window: the positions/balances table and the wallet containers, which are named and shown
per exchange (Spot, Futures, Quarterly; Unified Trading and Funding on Bybit). A
symbol with no PNG is drawn without an icon. Lookup strips a literal `1000` prefix, so
`1000PEPE` uses `pepe.png`.
