//! Source contracts for Mini App owner commands, the report-cache grant, viewer
//! isolation, and the page's label keys.
//!
//! These read the Telegram crate (`moon-tg`) as text, beside the page they serve. Each
//! oracle is the other side of a seam: the page's literal keys against the label
//! table and `locales/telegram.yml`, and the control-flow text a dropped guard
//! would delete. A string the test itself invented and then found is not an oracle.

use std::collections::BTreeSet;

use super::support::{
    assert_locale_key_in_three_languages, braced_body, read_core_src, read_tg_src,
};

/// Quoted `mini_*` keys in `app.js` that are whole label names.
///
/// A prefix such as `mini_fault_` is concatenated with a runtime kind and is not
/// itself a key. The shell keys are included: the page calls `tr` on them even
/// though they are inserted beside `MINI_LABEL_KEYS`.
fn page_label_keys(js: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut rest = js;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let key = &rest[..end];
        rest = &rest[end + 1..];
        if is_whole_mini_key(key) {
            keys.insert(key.to_string());
        }
    }
    keys
}

/// True when `key` is a complete `mini_*` label rather than a concatenated prefix.
fn is_whole_mini_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    key.starts_with("mini_")
        && !key.ends_with('_')
        && first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && !key.contains(' ')
}

/// String literals inside one Rust array or match, in source order of appearance.
fn quoted_strings(block: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut rest = block;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            break;
        };
        keys.insert(rest[..end].to_string());
        rest = &rest[end + 1..];
    }
    keys
}

/// The `MINI_LABEL_KEYS` array, and only that array.
fn mini_label_keys(telegram_rs: &str) -> BTreeSet<String> {
    let start = telegram_rs
        .find("const MINI_LABEL_KEYS: &[&str] = &[")
        .expect("moon-tg labels.rs defines MINI_LABEL_KEYS");
    let end = telegram_rs[start..]
        .find("];")
        .expect("MINI_LABEL_KEYS is a closed array");
    quoted_strings(&telegram_rs[start..start + end])
}

/// Label keys inserted by name in `telegram_labels`, outside `MINI_LABEL_KEYS`.
///
/// The three shell strings are composed next to the array. A page key has to be
/// in one of those two places or the WebApp renders an empty string.
fn inserted_mini_keys(telegram_rs: &str) -> BTreeSet<String> {
    let body = braced_body(telegram_rs, "fn telegram_labels(");
    let mut keys = BTreeSet::new();
    let mut rest = body;
    while let Some(marker) = rest.find(".to_string()") {
        let before = &rest[..marker];
        if let Some(close) = before.rfind('"')
            && let Some(open) = before[..close].rfind('"')
        {
            let key = &before[open + 1..close];
            if is_whole_mini_key(key) {
                keys.insert(key.to_string());
            }
        }
        rest = &rest[marker + ".to_string()".len()..];
    }
    keys
}

/// `moon-tg mini_app/mod.rs:mini_owner` must refuse a viewer and a
/// chat with no grant.
///
/// Mutation: the viewer arm returns `Ok(())`. A report viewer can then cancel
/// orders and arm Panic Sell. No grant returning `Ok(())` is the same hole for
/// an unpaired chat that still reached this function.
#[test]
fn mini_owner_rejects_viewer_and_absent_grant() {
    let source = read_tg_src("mini_app/mod.rs");
    let body = braced_body(&source, "fn mini_owner(");
    assert!(
        body.contains("Some(TelegramReportAccess::Viewer(_)) => Err(MiniAppApiError::Forbidden)"),
        "a viewer must be Forbidden before a money command runs"
    );
    assert!(
        body.contains("None => Err(MiniAppApiError::Rejected)"),
        "a chat with no grant must be Rejected before a money command runs"
    );
    assert!(
        !body.contains("Some(TelegramReportAccess::Viewer(_)) => Ok(())"),
        "a viewer arm that returns Ok lets that chat move money"
    );
}

/// `moon-tg mini_app/commands.rs:mini_cancel_order` must return NotFound
/// for an unlisted uid before `session.cancel_order`.
///
/// Mutation: delete the `order.uid == uid` miss. The uid is then sent to the
/// core even though the open-order list the page showed did not contain it.
#[test]
fn mini_cancel_unlisted_uid_is_not_sent() {
    let source = read_tg_src("mini_app/commands.rs");
    let body = braced_body(&source, "fn mini_cancel_order(");
    let listed = body
        .find("order.uid == uid")
        .expect("cancel must decide from the open-order uid");
    let miss = body[listed..]
        .find("return Ok(command_miss(CommandErrorDto::NotFound))")
        .map(|offset| listed + offset);
    let send = body.find("host.session_mut().cancel_order(");
    assert!(
        miss.is_some() && send.is_some() && miss.unwrap() < send.unwrap(),
        "an unlisted uid must return NotFound before session.cancel_order, so nothing is sent"
    );
}

/// `moon-tg mini_app/commands.rs:mini_panic_sell` must return NotFound
/// when the market is not on that core's open orders, before any toggle.
///
/// Mutation: delete the `order.market == market` miss. Panic Sell is then
/// toggled for a market the page did not list.
#[test]
fn mini_panic_unlisted_market_is_not_sent() {
    let source = read_tg_src("mini_app/commands.rs");
    let body = braced_body(&source, "fn mini_panic_sell(");
    let listed = body
        .find("order.market == market")
        .expect("panic must decide from the open-order market");
    let miss = body[listed..]
        .find("return Ok(command_miss(CommandErrorDto::NotFound))")
        .map(|offset| listed + offset);
    let toggle = body.find("host.toggle_panic_sell(");
    assert!(
        miss.is_some() && toggle.is_some() && miss.unwrap() < toggle.unwrap(),
        "a market with no order must return NotFound before toggle_panic_sell"
    );
}

/// `moon-tg mini_app/commands.rs:mini_panic_sell` must return success
/// without toggling when the market is already in the asked state.
///
/// Mutation: delete the `is_panic_armed(core, &market) == on` return. A second
/// tap sends another toggle and flips Panic Sell back off, or on, against the
/// state the page just showed.
#[test]
fn mini_panic_already_armed_does_not_toggle() {
    let source = read_tg_src("mini_app/commands.rs");
    let body = braced_body(&source, "fn mini_panic_sell(");
    let already = body
        .find("host.is_panic_armed(core, &market) == on")
        .expect("panic must compare the asked state with the armed state");
    let hit = body[already..]
        .find("return Ok(command_hit(Some(on)))")
        .map(|offset| already + offset);
    let toggle = body.find("host.toggle_panic_sell(");
    assert!(
        hit.is_some() && toggle.is_some() && hit.unwrap() < toggle.unwrap(),
        "an already-matching Panic Sell state must return ok before toggle_panic_sell"
    );
}

/// `moon-tg mini_app/reads.rs:mini_report` must serve a cached report
/// only when the cached grant still equals the chat's current grant.
///
/// Mutation: replace `(cached_access == &access, cached_access != &access)` with
/// `(true, false)`. A report cached under a wider grant is then served after the
/// chat is narrowed, for the rest of the minute.
#[test]
fn mini_report_cache_requires_same_grant() {
    let source = read_tg_src("mini_app/reads.rs");
    let body = braced_body(&source, "fn mini_report(");
    assert!(
        body.contains("(cached_access == &access, cached_access != &access)"),
        "a cache hit must compare the stored grant with the current one, and a mismatch must drop it"
    );
}

/// `moon-tg mini_app/mod.rs:visible_cores` must keep an empty viewer list empty.
///
/// Mutation: the viewer arm returns `true` instead of `ids.contains(&session.id)`.
/// A viewer whose grant lists no cores then sees every core's orders, balances,
/// and status.
#[test]
fn visible_cores_empty_viewer_grant_matches_nothing() {
    let source = read_tg_src("mini_app/mod.rs");
    let body = braced_body(&source, "fn visible_cores(");
    assert!(
        body.contains("TelegramReportAccess::Viewer(ids) => ids.contains(&session.id)"),
        "an empty viewer grant contains no session id, so the filter must use that list"
    );
    assert!(
        !body.contains("TelegramReportAccess::Viewer(_) => true")
            && !body.contains("TelegramReportAccess::Viewer(ids) => true"),
        "a viewer arm that returns true shows every core"
    );
}

/// `moon-tg mini_app/commands.rs:mini_cancel_order` must send through
/// `session.cancel_order`.
///
/// Mutation: replace `host.session_mut().cancel_order(core, uid)` with `Ok(())`. The
/// page reports the order cancelled and the core never receives the cancel.
#[test]
fn mini_cancel_goes_through_session_cancel_order() {
    let source = read_tg_src("mini_app/commands.rs");
    let body = braced_body(&source, "fn mini_cancel_order(");
    assert!(
        body.contains("host.session_mut().cancel_order("),
        "Mini App cancel must call session.cancel_order"
    );
}

/// `moon-tg mini_app/commands.rs:mini_panic_sell` must send through
/// `toggle_panic_sell`.
///
/// Mutation: replace `host.toggle_panic_sell(core, market.clone())` with `true`.
/// The page reports Panic Sell changed and the market's armed state does not.
#[test]
fn mini_panic_goes_through_toggle_panic_sell() {
    let source = read_tg_src("mini_app/commands.rs");
    let body = braced_body(&source, "fn mini_panic_sell(");
    assert!(
        body.contains("host.toggle_panic_sell("),
        "Mini App panic must call toggle_panic_sell"
    );
}

/// Every whole `mini_*` literal in `web/app.js` must be a key `telegram_labels`
/// puts on the page, and that key must have ru, en, and es in `locales/telegram.yml`.
///
/// Shell copy is inserted by name beside `MINI_LABEL_KEYS`; every other literal
/// has to be in the array. Mutation: delete `"mini_cancel"` from
/// `MINI_LABEL_KEYS`. The cancel button then renders as an empty string while
/// the Russian, English, and Spanish strings still exist in the locale file.
#[test]
fn mini_page_literals_are_wired_and_translated() {
    let page = page_label_keys(&read_core_src("telegram/web/app.js"));
    let telegram_rs = read_tg_src("labels.rs");
    let wired: BTreeSet<String> = mini_label_keys(&telegram_rs)
        .into_iter()
        .chain(inserted_mini_keys(&telegram_rs))
        .collect();
    assert!(
        page.len() >= 10,
        "the page scanner found too few mini label literals to be the WebApp copy"
    );
    assert!(
        page.contains("mini_cancel"),
        "the page must still ask for the cancel label this mutation removes"
    );
    let missing: Vec<&String> = page.difference(&wired).collect();
    assert!(
        missing.is_empty(),
        "app.js literals missing from MINI_LABEL_KEYS and telegram_labels inserts: {missing:?}"
    );
    for key in &page {
        assert_locale_key_in_three_languages("telegram.yml", &format!("telegram.{key}"));
    }
}

/// Ping and CPU units on the cores tab must come from the label map.
///
/// Mutation: put the unit back in the script as `" ms"` or `+ "%"`. Russian
/// then shows the Latin abbreviation, and the percent sign is no longer a
/// locale value the page can resolve.
#[test]
fn mini_core_units_come_from_labels() {
    let js = read_core_src("telegram/web/app.js");
    assert!(
        js.contains("\"mini_unit_ms\""),
        "ping must read its unit from mini_unit_ms"
    );
    assert!(
        js.contains("\"mini_unit_pct\""),
        "cpu must read its unit from mini_unit_pct"
    );
    assert!(
        !js.contains("\" ms\""),
        "a hard-coded ms unit bypasses the Russian мс label"
    );
    assert!(
        !js.contains("+ \"%\""),
        "a hard-coded percent sign is not in the label map"
    );
}

/// `mini_app.rs:balance_text` must format through `usd_grouped_cents`.
///
/// Mutation: call `fmt::usd_grouped` again. Exchange rows go back to one or
/// two decimals (`10 000.0$` beside `170 293.47$`) while the hero uses the
/// same helper, so the tab cannot keep a single width.
#[test]
fn mini_balance_text_uses_fixed_cents() {
    let source = read_tg_src("mini_app/dto.rs");
    let body = braced_body(&source, "fn balance_text(");
    assert!(
        body.contains("fmt::usd_grouped_cents("),
        "Mini App balances must format through usd_grouped_cents"
    );
    assert!(
        !body.contains("fmt::usd_grouped("),
        "usd_grouped trims the hundredths this tab has to show"
    );
}

/// The day table, balance figures, core names, and Panic Sell button keep the
/// layout the narrow Mini App popup needs.
///
/// Mutation: drop `line-clamp: 2`, the day table's 16px cell inset, `.balance-figure`,
/// or `white-space: nowrap` on `.order-actions .cmd`. The day figures meet the card
/// edge, core names ellipsize on one line, balance amounts stay pale, or
/// «Выключить Panic Sell» breaks onto two lines inside the button. Stretching
/// `.order-actions .cmd` with `flex: 1 1 auto` brings back the full-width
/// stacked pair the compact row replaced.
#[test]
fn mini_app_css_keeps_the_narrow_popup_layout() {
    let css = read_core_src("telegram/web/app.css");
    assert!(
        css.contains("line-clamp: 2"),
        "a core name must wrap to two lines instead of one ellipsis"
    );
    assert!(
        css.contains(".day-table td {\n    padding: 7px 16px;"),
        "the day table must use the card's 16px inner inset"
    );
    assert!(
        css.contains(".balance-figure {\n    color: var(--ink);"),
        "balance amounts must use the Telegram text color"
    );
    assert!(
        css.contains(".core-row .metrics {\n    max-width: none;"),
        "the ping column must size to its text instead of half the row"
    );
    let actions = css
        .split(".order-actions {")
        .nth(1)
        .expect("order actions must have a row rule");
    let row = actions.split('}').next().unwrap_or("");
    assert!(
        row.contains("flex-wrap: nowrap;") && row.contains("justify-content: flex-end;"),
        "cancel and Panic Sell must share one right-aligned row"
    );
    let cmd = css
        .split(".order-actions .cmd {")
        .nth(1)
        .expect("order command buttons must have a rule");
    let body = cmd.split('}').next().unwrap_or("");
    assert!(
        body.contains("min-height: 44px;") && body.contains("white-space: nowrap;"),
        "Panic Sell must stay on one line and keep a 44px tap height"
    );
    assert!(
        !body.contains("flex: 1 1 auto;"),
        "order actions must stay compact instead of stretching across the row"
    );
}

/// `web/app.js:ensureCollapse` starts the groups of every pane collapsed: Orders,
/// Strategies, Cores and Balances.
///
/// Mutation: restore `collapse[pane][slot] = count > 20 && !problem`, or drop
/// the `collapseUser` skip. Small order groups then open on first paint and on
/// every poll the user has not toggled. Dropping the cores or balances call
/// brings back the wall of open exchange groups the owner asked to fold.
#[test]
fn mini_list_groups_start_collapsed_until_the_user_toggles() {
    let js = read_core_src("telegram/web/app.js");
    let ensure = braced_body(&js, "function ensureCollapse(");
    assert!(
        ensure.contains("if (collapseUser[pane][slot]) continue;"),
        "a group the user toggled must keep that choice across a refresh"
    );
    assert!(
        ensure.contains("collapse[pane][slot] = true;"),
        "an untouched group must start collapsed"
    );
    assert!(
        !ensure.contains("count > 20") && !ensure.contains("isProblem"),
        "row count and problems must not decide the default"
    );
    assert!(
        js.contains("ensureCollapse(\"orders\", orders, orderCoreKey);")
            && js.contains("ensureCollapse(\"strategies\", withRows, strategyCoreKey);")
            && js.contains("ensureCollapse(\"cores\", cores, null);")
            && js.contains("ensureCollapse(\"balances\", perCore, null);"),
        "orders, strategies, cores and balances must seed the collapsed default"
    );
    let append = braced_body(&js, "function appendGroups(");
    assert!(
        append.contains("var collapsed = !!collapse[pane][slot];"),
        "a group must draw folded exactly when its collapse slot says so"
    );
}

/// Collapsed group headers must keep the summary the open rows used to show.
///
/// Mutation: drop `coreGroupSummary` or `balanceGroupSummary` from the
/// `appendGroups` call. A collapsed exchange then shows only its name, which
/// is the popup the user could not read without opening every group.
///
/// An order group's PnL sums the finite figures and shows nothing when none
/// exist. Mutation: set `known = false` and `break` on the first order
/// without a figure. A core with open positions plus one unfilled order
/// then loses those positions' PnL. Mutation: draw a dash for an unvalued
/// core. The header then shows "—" where a PnL is expected.
#[test]
fn mini_collapsed_groups_carry_their_summary() {
    let js = read_core_src("telegram/web/app.js");
    let cores = braced_body(&js, "function coreGroupSummary(");
    assert!(
        cores.contains("\"mini_cores_online\"") && cores.contains("coreProblem("),
        "a core group must say how many are online and mark a faulted group"
    );
    let balances = braced_body(&js, "function balanceGroupSummary(");
    assert!(
        balances.contains("total_text"),
        "a balance group must show the exchange total the page already received"
    );
    let known = braced_body(&js, "function knownPnl(");
    assert!(
        known.contains("known += 1") && !known.contains("known = false"),
        "an order group must sum every finite PnL, not stop at the first unvalued order"
    );
    let orders = braced_body(&js, "function orderGroupSummary(");
    assert!(
        orders.contains("knownPnl(items)")
            && orders.contains("mini_orders_n_")
            && orders.contains("applyMoney(fig, \"num\", text, pnl.sum)")
            && !orders.contains("\\u2014"),
        "an order group labels its count and shows PnL only when some order is valued"
    );
}

/// The order row's entry-to-mark percent must be the desktop table's figure.
///
/// Mutation: format `change_text` from `entry_text` in the page. Adaptive
/// prices drop cents at 1000, so the percent on the row would disagree with
/// the orders table for the same order.
#[test]
fn mini_order_change_percent_matches_the_desktop_table() {
    let source = read_tg_src("mini_app/dto.rs");
    let body = braced_body(&source, "fn order_dto(");
    assert!(
        body.contains("order_pnl_pct(") && body.contains("fmt::signed_pct("),
        "Mini App change percent must use order_pnl_pct and signed_pct"
    );
    assert!(
        body.contains("change_text:"),
        "the page reads change_text; dropping the field leaves the percent blank"
    );
}

/// Tab and period changes, and a confirmed money command, must use Telegram
/// haptics and the header BackButton, each behind a feature check.
///
/// Mutation: call `HapticFeedback.impactOccurred` or `BackButton.show` without
/// the `typeof` guard. An older Telegram client throws and the command or the
/// tab switch stops.
#[test]
fn mini_telegram_feedback_is_feature_detected() {
    let js = read_core_src("telegram/web/app.js");
    assert!(
        js.contains("selectionChanged") && js.contains("impactOccurred(\"medium\")"),
        "tab and period switches use selectionChanged; a sent money command uses a medium impact"
    );
    assert!(
        js.contains("notificationOccurred"),
        "a money command still reports success and error with notificationOccurred"
    );
    assert!(
        js.contains("BackButton") && js.contains("typeof button.show"),
        "the header back button must be feature-detected"
    );
    assert!(
        js.contains("header.clientWidth <= 300"),
        "the updated-time word is dropped at 300px so the title row does not wrap"
    );
    let badge = braced_body(&js, "function balanceBadge(");
    assert!(
        badge.contains("state === \"live\"") && badge.contains("return null"),
        "a live balance row must not wear the fresh chip"
    );
}

/// `web/app.js:loadTab` must keep painted data when a background read fails,
/// except on 401 and 403.
///
/// Mutation: drop `res.status !== 401 && res.status !== 403` from the stale
/// guard. An expired session then keeps showing old rows as if they were fine
/// instead of asking to relaunch. Mutation: call `showError` in place of
/// `showStale` in the stale branch, or reset `hasData[name]` there. One 503
/// during the 2 s poll then blanks the Cores or Orders list.
#[test]
fn mini_failed_background_read_keeps_data_but_not_on_auth_failure() {
    let js = read_core_src("telegram/web/app.js");
    let load = braced_body(&js, "function loadTab(");
    let failure = braced_body(load, "if (!res.ok || !res.data)");
    let stale = braced_body(failure, "if (silent && hasData[name]");
    let guard = &stale[..stale.find('{').expect("stale branch has a body")];
    assert!(
        guard.contains("res.status !== 401") && guard.contains("res.status !== 403"),
        "401 and 403 must skip the stale branch so the session error is shown: {guard}"
    );
    assert!(
        stale.contains("showStale(") && stale.contains("return;"),
        "a failed background read must show the stale bar and stop there"
    );
    assert!(
        !stale.contains("showError(") && !stale.contains("hasData[name] = false"),
        "the stale branch must keep the painted data"
    );
    let after = &failure[failure.find(stale).expect("stale branch inside failure") + stale.len()..];
    assert!(
        after.contains("hasData[name] = false;") && after.contains("showError(paneBody(name)"),
        "every other failure, 401 and 403 included, takes the full error path"
    );
}

/// `web/app.js:loadTab` clears the busy flag only for the read that owns it.
///
/// Mutation: replace `if (inFlight[name] === ticket) {` with an unconditional
/// block. A cancelled older read then clears a newer read's flag and spinner,
/// so after a period switch the refresh button re-enables mid-read and a tap
/// starts a duplicate read.
#[test]
fn mini_stale_read_does_not_clear_a_newer_reads_busy_flag() {
    let js = read_core_src("telegram/web/app.js");
    let load = braced_body(&js, "function loadTab(");
    assert!(
        load.contains("inFlight[name] = ticket;"),
        "each read must stamp its own ticket"
    );
    let owned = braced_body(load, "if (inFlight[name] === ticket)");
    assert!(
        owned.contains("inFlight[name] = null;") && owned.contains("setSpinning(false);"),
        "only the owning read clears the flag and stops the spinner"
    );
    assert_eq!(
        load.matches("inFlight[name] = null").count(),
        1,
        "the busy flag must not be cleared outside the ticket check"
    );
}
