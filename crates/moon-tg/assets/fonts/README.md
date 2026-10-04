# Deal chart faces

The Telegram deal chart (`crates/moon-tg/src/deal_chart`) draws its text with these two faces,
compiled into the binary: a server has no fonts of its own.

- `GeistMono-Regular.ttf` — the terminal's own face, copied from MoonUI
  (`assets/fonts/geist-mono`). SIL Open Font License 1.1, `GeistMono-OFL.txt`.
- `NotoSansSC-Subset.ttf` — the fallback for coin names in CJK script, which Geist Mono lacks.
  Noto Sans SC (`github.com/google/fonts`, `ofl/notosanssc/NotoSansSC[wght].ttf`) instanced at
  weight 400 and cut to the GB2312 set (6763 hanzi) plus CJK punctuation (U+3000–U+30FF, kana
  included) and fullwidth forms (U+FF00–U+FFEF): 2.2 MB instead of 17 MB. TrueType outlines
  (`glyf`), no composite glyphs, cmap format 4. SIL Open Font License 1.1, `NotoSansSC-OFL.txt`.

Both must stay TrueType (`glyf`) faces with a format 4 character map: that is what the chart's
own reader (`deal_chart/font/ttf.rs`) reads.
