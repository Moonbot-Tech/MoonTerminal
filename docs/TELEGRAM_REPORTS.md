# Telegram chat reports

Pair your private chat in Settings -> Telegram, then send `/start`. A short welcome installs a
persistent reply keyboard with period choices and Help. The separate report opens as
an exchange summary with its inline keyboard already attached. Both messages remain editable only
where Telegram permits it: the welcome is not reused as a report.

The bottom reply keyboard opens a global overview for the selected period. Buttons beneath a report
work on that report: select an exchange to see its cores, switch to days, go back to all exchanges,
or page. Tap a period again in the reply keyboard for fresh data; there is no inline Refresh button.
Period choices are not duplicated inline. Button glyphs distinguish those actions.
Mini App and its tunnel are not needed. Keep MoonTerminal running for history synchronization.

Groups without trades are hidden before pagination. Zero-profit trades remain visible.
Pages contain at most six active rows. Core names span the table width;
native currency amounts and averages are in an expandable two-column table. Dollar-denominated amounts
use two decimals; crypto-denominated amounts retain up to eight. Calculation guidance is in Help; average-coverage counts remain in the monetary details only
when rows were excluded.
Exchange membership uses reported venue identity, including market type and HIP-3 DEX.
Historical cores without known membership appear under Unidentified, never a guessed exchange.

| Command | Result |
| --- | --- |
| `/report` or `/today` | Today by exchange |
| `/hour` | Current calendar hour |
| `/yesterday` | Previous calendar day |
| `/month` | Current month to now |
| `/lastmonth` | Previous calendar month |
| `/daily` | Current month by day |
| `/report 2026-09-01 2026-09-10` | Custom period by exchange |
| `/daily 2026-09-01 2026-09-10` | Custom period by day |

Custom dates are inclusive, at most 366 days. Calendar boundaries follow the terminal clock's
selected time zone, including daylight-saving changes. Paging and changing the view retain the
resolved UTC bounds; A new reply-keyboard request resolves the period again. Changing the terminal's time
zone changes the report's display and daily grouping on the next request.

Reports cover all bots retained in local history, including bots no longer configured. They
include closed real trades and exclude emulator and deleted trades. Offline bots' local history
may be incomplete. The final Total row covers the whole period, including groups on other pages.

USDT figures use the terminal's historical valuation at trade time. A missing rate or unknown
currency makes the complete USDT result unavailable; it does not become zero or a partial total.
Native currency subtotals remain separate. Average order calculations use Report's counted entry
spend and currency rules. These are historical reports, not balances or unrealized positions.

Formatting uses Telegram's [Rich Messages](https://core.telegram.org/bots/api#rich-message-formatting-options),
introduced in Bot API 10.1. Use a current Telegram client. This feature requires no additional
bot, public website, or trading permissions.

Exchange buttons use brand-colored circles. Today omits the redundant daily view.
Main-table USDT amounts carry a `$` suffix; native-currency details keep their own ticker.

Core names occupy a full-width row above money columns; native details use two money columns.
Help is a standalone rich message with collapsed commands, calculation notes, and Mini App guidance.
Successful reply-button reports and Help replace the previous tracked answer by sending first,
then deleting it. Tracking is persisted per bot identity and survives service and terminal restarts.
Failed delivery retains the previous answer. Answers at the 48-hour limit (with a one-minute margin) are skipped; missing or undeletable messages are quiet cleanup no-ops.

Persistent reply navigation belongs to a separate permanent welcome message.
The first rich response without a saved menu owner installs it; `/start` can reinstall it.
Only report and Help message IDs enter cleanup tracking; Help never owns the reply keyboard.

Only IDs and original Telegram timestamps are stored in the per-bot chat-history JSON.
Atomic persistence completes before deleting a replaced answer; a failed save retains the old message.
The history cache never grants authorization and contains no token or report content.
Messages sent before this persistence feature cannot be recovered from Telegram history.
