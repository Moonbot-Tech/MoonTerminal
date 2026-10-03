# Telegram chat reports

Pair your private chat in Settings -> Telegram, then send `/start`. A short welcome installs a
reply keyboard with period choices and Help; it can be folded away and opened again with the
keyboard icon, and Android's Back folds it before leaving the chat. The bot also registers its
commands, so the chat's menu button lists them whenever it is not the Mini App's. The separate report opens in the bot's
report view (by exchange unless changed) with its inline keyboard already attached. Both messages remain editable only
where Telegram permits it: the welcome is not reused as a report.

The bottom reply keyboard opens a global overview for the selected period. Buttons beneath a report
work on that report: select an exchange to see its cores, switch to days, go back to all exchanges,
or page. Tap a period again in the reply keyboard for fresh data; there is no inline Refresh button.
Period choices are not duplicated inline. Button glyphs distinguish those actions.
Mini App and its tunnel are not needed. A terminal-hosted bot needs MoonTerminal running for
history synchronization; a station-hosted bot runs independently and reads the station's history.

Groups without trades are hidden before pagination. Zero-profit trades remain visible.
Every view (exchanges, cores, days) lists all its active rows in one message; pagination appears
only when the rendered rich message would exceed Telegram's 32768-character or 500-block limit,
and then each page holds the largest of a fixed set of page sizes that still fits, so Next and
Previous keep their rows when a trade closes between two presses. A core is one table row: a long name keeps its
first 8 and last 15 characters, and the full name is in the details. Native currency amounts and
averages are in an expandable two-column table. Dollar-denominated amounts
use two decimals; crypto-denominated amounts retain up to eight. Calculation guidance is in Help; average-coverage counts remain in the monetary details only
when rows were excluded.
Exchange membership uses reported venue identity, including market type and HIP-3 DEX.
Historical cores without known membership appear under Unidentified, never a guessed exchange.

| Command | Result |
| --- | --- |
| `/report` or `/today` | Today |
| `/hour` | Current calendar hour |
| `/yesterday` | Previous calendar day |
| `/month` | Current month to now |
| `/lastmonth` | Previous calendar month |
| `/daily` | Current month by day |
| `/report 2026-09-01 2026-09-10` | Custom period |
| `/daily 2026-09-01 2026-09-10` | Custom period by day |

Period commands and buttons open in the bot's report view; `/daily` always splits by day, and a
one-day period always opens by exchange. Custom dates are inclusive, at most 366 days. Calendar boundaries follow the terminal clock's
selected time zone, including daylight-saving changes. Paging and changing the view retain the
resolved UTC bounds and the period basis the report was read on; a new reply-keyboard request
resolves the period again. Changing the terminal's time zone changes the report's display and
daily grouping on the next request; a station-hosted bot receives the new zone without a restart.

## Bot menu

Settings -> Telegram -> Bot menu lays out the reply keyboard and the Report section as rows of
buttons: a tick shows a button, "new row" starts a row, the arrows change the order. The same box
sets the view reports open in (by exchange, by core, by day) and whether periods count trades by
close time (the default, as the terminal's Report) or by open time; a report read by open time
says so under its period. A terminal-hosted bot saves these with Save; a station-hosted bot takes
them with "Apply on the server", without a restart. Chats get a changed keyboard with the bot's
next message; a button already on an older keyboard keeps working within the chat's role.

The owner also has a Settings button (and `/settings`): a menu under one message to show or hide
buttons, pick the report view and the period basis, switch the Mini App (a terminal-hosted bot;
a station's is switched in the terminal's Settings), set this chat's notifications — trade
cards, core down/back with a delay, the daily summary and its hour — and, on a station, open its
status. Each switch saves at once; button order and rows stay in the terminal's Settings, trade
thresholds and cores in the Mini App's Settings tab. A change made in the chat reaches an open
terminal Settings window only where that window had not been edited.

Each chat's notifications can also be set in Settings -> Telegram -> Chats: open a chat for
its trade cards (cores, minimum volume, profit and loss thresholds), core down/back notices with
their delay, and the daily summary with its time. "Save notifications" saves that chat alone,
at once for a terminal-hosted bot and on the server for a station-hosted one; a change made
meanwhile from the Mini App or the chat is refused rather than overwritten.

The Report button opens a menu under one message, which turns into the report pressed. Custom
period offers the last 7 days, the last 30 days, last week, and a calendar: the first press picks
the first day, the second the last one, up to a year later.

Settings -> Telegram -> Chats and core access assigns one owner and any number of read-only
viewers. The first paired chat is the owner, including when upgrading an older flat pairing list;
other chats start with no assigned cores. The owner can read all bots retained in local history,
including bots no longer configured. Viewers can read only explicitly assigned cores. Their totals,
exchange groups, daily views and old callback buttons all use those same saved permissions.

Expand a chat to give it a local name, search cores by number or name, and select its available
cores. Archived cores are loaded from local history and can also be assigned. Selecting all current
cores captures that list: cores added later are not automatically shared. Press Save to apply names,
roles, assignments or an individual unpairing. Ownership transfer requires confirmation and leaves
the previous owner as a viewer with no assigned cores. Reset pairing revokes every chat and role.
Changing saved permissions cancels pending deliveries through the previous service generation;
already delivered Telegram messages are not recalled. Existing Telegram functionality remains
reporting: assigning the owner role does not add trading or core-control commands in the chat.
On a station-hosted bot, the owner also has a Status reply button and `/status`. Its status answer
offers Update when a newer release contains the station binary, and a way to Settings. Status, Update and Settings are
refused for viewers; a terminal-hosted bot cannot run either station action.

Reports
include closed real trades and exclude emulator and deleted trades. Offline bots' local history
may be incomplete. The final Total row covers the whole period, including groups on other pages.

USDT figures use the terminal's historical valuation at trade time. A missing rate or unknown
currency makes the complete USDT result unavailable; it does not become zero or a partial total.
Native currency subtotals remain separate. Average order calculations use Report's counted entry
spend and currency rules. These are historical reports, not balances or unrealized positions.

Formatting uses Telegram's [Rich Messages](https://core.telegram.org/bots/api#rich-message-formatting-options),
introduced in Bot API 10.1. Use a current Telegram client. This feature requires no additional
bot, public website, or trading permissions.

Exchange buttons use brand-colored circles. A one-day report omits the redundant daily view.
Main-table USDT amounts carry a `$` suffix; native-currency details keep their own ticker.

Native details use two money columns, with each full core name above its figures.
Help is a standalone rich message with collapsed commands, calculation notes, and Mini App guidance.
Successful reply-button reports and Help replace the previous tracked answer by sending first,
then deleting it. Tracking is persisted per bot identity and survives service and terminal restarts.
Failed delivery retains the previous answer. Answers at the 48-hour limit (with a one-minute margin) are skipped; missing or undeletable messages are quiet cleanup no-ops.

Persistent reply navigation belongs to a separate welcome message. The first rich response
without a saved menu owner installs it; `/start` can reinstall it; a changed keyboard (another
menu, role or language) sends a new one and deletes the previous, so the chat keeps one. A
reply-button press is deleted once the bot has answered it; a press left unanswered stays, and
typed commands are never deleted. Tidying is one quiet attempt after the answer: it is never
retried, skipped under a rate limit, and never changes the bot's status.
Only report and Help message IDs enter cleanup tracking; Help never owns the reply keyboard.

Only IDs and original Telegram timestamps are stored in the per-bot chat-history JSON.
Atomic persistence completes before deleting a replaced answer; a failed save retains the old message.
The history cache never grants authorization and contains no token or report content.
Messages sent before this persistence feature cannot be recovered from Telegram history.

## Notifications

The Mini App «Настройки» tab is where a paired chat turns on messages the bot sends on its own.
Every switch is off until that chat saves it. Each chat has its own settings. A viewer is limited
to the cores that chat can see. The owner can hear about every core the host keeps.

Three kinds of message can be sent. They are ordinary bot messages, not rich reports:

- Closed trades, with optional filters: which cores, a minimum volume in USD, a profit of at
  least some USD, and a loss of at least some USD. Each trade is announced once. Only a trade
  that closes after the switch is turned on is eligible, and the dedup window is 72 hours.
  A close older than that window is not announced.
- Core down and back. After a core stays disconnected for the chosen number of minutes, the
  chat receives one down message. When that core connects again, the chat receives one back
  message. A core that leaves the configured set is forgotten: it does not stay announced as
  down, and no back message is sent for it. If it returns and is lost again, the delay starts
  over.
- A daily summary at a chosen time in the display time zone, the same zone chat reports use.
  If the bot is down at that time, it sends the summary once later on that same local day, and
  not for an earlier day.

Accepted messages wait in a durable outbox and are delivered after a restart if Telegram had not
accepted them yet. A network timeout can leave the outcome ambiguous, so a rare duplicate is
possible. The sender paces a private chat at one message per second, and a group or channel (a negative chat id) at one message per three seconds.
