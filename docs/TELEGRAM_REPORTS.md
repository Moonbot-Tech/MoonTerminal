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
averages are in an expandable two-column table. The main table starts with column titles and
ends with the whole-period Total row, with every total cell bold. Its caption keeps the view and
period on one line; an automatic report uses its own title and zone. Dollar-denominated amounts
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

Settings -> Telegram -> Bot menu lays out the reply keyboard as rows of buttons: a tick shows a
button, "new row" starts a row, the arrows change the order. The Report section always shows all
its periods; the Mini App opens from the chat's menu button, not from the keyboard. The same box
sets the view reports open in (by exchange or by core; by day is a button under the report, a
menu button and `/daily`) and whether periods count
trades by close time (the default, as the terminal's Report) or by open time; a report read by open
time says so under its period. A terminal-hosted bot saves these with Save; a station-hosted bot
takes them with "Apply on the server", without a restart. Chats get a changed keyboard with the
bot's next message; a button already on an older keyboard keeps working within the chat's role.

The view by core lists the cores under the terminal's saved core groups, as the Profit Monitor
does: one group header carrying its total, then its cores; groups by name, "Ungrouped" last. A
core in two groups is listed under both and counts in both subtotals, while the report's total
counts it once; a group of one core carries that core's total, and when one group would hold every core the
list stays flat. A report long enough to page repeats a group's header on the page that continues
it. A terminal-hosted bot reads the terminal's groups; a station keeps the set it
was last sent. The station's bot menu shows whether its groups are this terminal's and offers "Send
groups to the station" when they are not — never on its own, so a terminal without groups cannot
wipe the set another one sent. Moving the bot to the station takes this terminal's groups along.

Settings -> Station -> Cores on the station compares cores by address, using the station's own
identities for Chats grants. Save in Connections never changes the station. The block shows
matching cores, cores only here, cores only on the station, and changed names or keys; row buttons
add a core or send its name or key. Send changes sends only additions and changes, leaving
station-only cores untouched. Removing a station-only core requires two inline clicks and keeps
its reports on the station; the final station core cannot be removed. An older station shows an
update notice until it supports the listing. Installation sends only the picked cores.

The owner also has a Settings button (and `/settings`): a menu under one message to show or hide
buttons, pick the report view and the period basis, switch the Mini App (a terminal-hosted bot;
a station's is switched in the terminal's Settings), set this chat's notifications — trade
cards, core down/back with a delay, the automatic reports — and,
on a station, open its
status. Each switch saves at once; button order and rows stay in the terminal's Settings, trade
thresholds and cores in the terminal's Settings and the Mini App's Settings tab. A change made in
the chat reaches an open terminal Settings window only where that window had not been edited.

Every screen under the bot's Settings menu names its host: "Bot of this terminal" or "Station
bot". In Settings -> Telegram, the bot and menu/notifications section titles distinguish the
terminal's bot from the station's bot; the station title includes its configured host. These
are independent bots with independent chat settings: changing one does not change the other.

Each chat's notifications can also be set in Settings -> Telegram, in the right column of the
"Bot menu and notifications" box: pick a chat, then its trade cards (cores, minimum volume, profit
and loss thresholds, the dollar follow-up), core down/back notices with their delay, and the
automatic reports. "Save notifications" saves that chat alone,
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
already delivered Telegram messages are not recalled. Viewers only read; the owner can also
command the cores from the chat (Control, below).

The owner's Control button (and `/control`) — hidden until shown in the bot menu — opens the cores
a page at a time with "All cores" on top. A core's card shows its link, trading, auto detect, open
positions and its own blacklist, and starts or stops trading, switches auto detect, panic-sells
every open position, cancels its buy orders still waiting to fill (per market, positions and their
sells stay), reconnects, and puts a typed coin on or off the core's blacklist (the next message is
the coin, for two minutes; the question lists the blacklist as it stands). From there the core's
open positions each panic-sell, go on the core's or the strategy's blacklist, or have their market
banned for 1 h, 4 h, 24 h or 3 days; its strategies switch on and off. Stopping or starting all
cores, panic sell and cancelling buys ask for confirmation first. A command the core still has to
confirm shows ⏳, and the bot redraws the same message once the core reports the asked state, or
says it did not. Commands go only to connected cores, through the same calls as the Mini App's. On
a station, showing Control switches it to the full feed profile (orders, strategies, trading state;
more traffic and CPU) from its next start.
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

## Message look

Settings -> Telegram has a "Bot message builder" box under the bot menu, with Card and Report
tabs and a preview made from synthetic examples. A station-hosted bot stores the layout with its
bot settings **on the station**: every terminal edits the same set. Press "Save on the station"
to apply it. A terminal-hosted bot stores it in its own config through the global Save button.

The Card tab arranges fields into lines. Drag fields and lines to reorder them, hide fields in
the tray, or restore them from it; the move and hide buttons and tray also work with the keyboard.
"Coin first" is the default, matching the existing card. "Core first, as in MoonBot" starts with
`Name: <core>`. The coin and core hashtag switches affect cards only: core down/back notices
always keep their hashtags.

The Report tab keeps the first column fixed and lets you reorder or hide Profit, Trades,
Average % and Volume. At least one of these columns stays visible; all four may not fit a phone.
Volume shows a native amount only for a complete single-currency scope.
The total goes at the Bottom by default, or at the Top. Separation is Band by default, a shaded
row with bold text and values; Spacer + band adds an empty row beside it. There is no line option
because Telegram rich tables cannot draw a rule inside a table. Group rows use Band by default,
or Bold left for bold cells with a left-aligned name.

A layout saved by a newer terminal still renders the fields and columns this version knows.
An older station or terminal may ignore or reset the layout when it saves the bot settings.

## Notifications

The Mini App «Настройки» tab is where a paired chat turns on messages the bot sends on its own.
Every switch is off until that chat saves it. Each chat has its own settings. A viewer is limited
to the cores that chat can see. The owner can hear about every core the host keeps.

Three kinds of message are ordinary bot messages, not rich reports (automatic reports, below, are
rich):

- Closed trades, with optional filters: which cores, a minimum volume in USD, a profit of at
  least some USD, and a loss of at least some USD. Each trade is announced once. Only a trade
  that closes after the switch is turned on is eligible, and the dedup window is 72 hours.
  A close older than that window is not announced. The card leaves within about five seconds of
  the trade reaching the report replica. Its first line shows the sign mark, the coin hashtag in bold,
  the profit and percent, the entry volume when known, and the holding duration, separated by
  middle dots. Profit and volume use the trade's own currency (`+0.00012 BTC`, `+3.3 USDC`;
  outside a USD stablecoin the dollar profit follows once the USDT valuation has it). Without
  native volume, the valued entry volume prints in whole dollars. The second line is the core hashtag,
  and the third is the strategy in italics. The filters are in USD: the valuation's figure, or a USD stablecoin's own amount taken 1:1.
  A trade in another currency that a filter needs to judge waits up to five minutes for its
  valuation; after that it is sent with a line saying its thresholds were not checked. With the
  dollar follow-up on (terminal Settings only), a card sent before its valuation gets the dollar
  value written into it once it arrives, within a day.
- Core down and back. After a core stays disconnected for the chosen number of minutes, the
  chat receives one down message. When that core connects again, the chat receives one back
  message. A core that leaves the configured set is forgotten: it does not stay announced as
  down, and no back message is sent for it. If it returns and is lost again, the delay starts
  over. Both notices use the same core hashtag as its trade cards.
- What the cores themselves would send to their own Telegram, which does not come over the
  wire: a trade opened, when its strategy has "Report trades to Telegram" on, and a detect, when
  its strategy has "Report to Telegram" on. Two switches, "Trade opened" and "Detect", in the
  terminal's Settings and the bot's Settings -> Notifications; emulator trades are included, and
  the chat adds no filters of its own. These events keep the core's hashtag and a colon first,
  then each event with its coin as a hashtag and the strategy on the line below; consecutive
  events of one core share its name. The first event after a quiet spell goes at once; what
  follows within 2 seconds (5 in a group, which Telegram takes slower) is merged into one message,
  so a burst of detects is one list; a long list is cut and counted. Only what happens
  while the bot runs is relayed: nothing older than two minutes, each trade once, and nothing for
  a chat whose queue Telegram has not drained for five minutes.

Coin and core hashtags are always shown on these pushes and chart captions. Tapping one searches
the chat for that coin or core. The first 64 Unicode scalars keep their case, letters and digits;
spaces, dashes, dots, slashes and other punctuation become underscores. Cyrillic letters stay as
they are. A name without a letter (including a digits-only or empty name) keeps its original
escaped text without `#`. The same mapping and length cap apply to every push above.

Deal charts are a picture of a closed trade, sent apart from the trade cards — a chat may have the
cards off and the pictures on. They are switched per chat in the terminal's Settings -> Telegram,
in the station's bot box (the Mini App does not show them, and its save leaves them as they are),
with two thresholds of their own: a profit of at least some USD, a loss of at least some USD; with
neither set every trade gets one. A picture waits for the trade's dollar value (a USD stablecoin
taken 1:1, another currency up to five minutes for its valuation) and is not sent without it. It
shows the trade's own window — at least 10 seconds before the entry (a third of a longer trade, as
far as the station's tape recorder keeps) and 3 seconds after the exit — with every print of the
tape as a cross in the colour of its side, the volume, the entry and exit orders as the steps the
core archived for them, the stop, both fills with their prices, the result, the position size and
the core's day so far. Its caption is the trade card. The picture is drawn by the station from its
own recorded tape, without a graphics library; a terminal-hosted bot draws one only while the
terminal records tape. A picture decided but not yet drawn when the process stops is not sent.

Automatic reports are the rich report a menu button opens, sent by the bot on its own. They are
switched per chat from the bot's Settings -> Notifications or the terminal's Settings -> Telegram ->
Bot menu and notifications (the Mini App does not show them, and its save leaves them as they are):

- Each hour separately, at the top of every hour in the display time zone, for the hour that just ended. Every
  hourly report stays in the chat.
- Today, every hour: the running summary from midnight is replaced at the start of each hour;
  at midnight, the whole day that just ended.
- Month, at midnight: from the 1st to the end of the day that just ended; on the 1st, the whole
  month that just ended.

Turning off the separate hourly reports does not turn off Today's hourly updates. The compact
toggle labels show these schedules in both the bot's Notifications menu and Settings -> Telegram;
the section hint explains replacement and the midnight summary.

A new today or month report replaces the chat's previous one of its kind. Each opens in the bot's
report view and counts on its period basis; a line on top names it, the zone offset and the basis,
and its buttons keep the same frozen period. Reports with no trades are sent too. A report turned
on starts at the next slot. A terminal-hosted bot sends only while the terminal runs; after a pause
each kind sends its latest slot once, never the slots it missed. A station-hosted bot sends around
the clock.

Accepted messages wait in a durable outbox and are delivered after a restart if Telegram had not
accepted them yet. A network timeout can leave the outcome ambiguous, so a rare duplicate is
possible. The sender paces a private chat at one message per second, and a group or channel (a negative chat id) at one message per three seconds.
