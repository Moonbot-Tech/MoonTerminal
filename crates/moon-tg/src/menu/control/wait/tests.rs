//! Pins for the Control section's wait for the core and the redraw that ends it.

use moon_core::session::{CoreRunState, RunSwitch};
use moon_core::telegram::api::{InlineKeyboardMarkup, ReplyMarkup};
use moon_core::telegram::menu_action::ControlAction;
use moon_core::telegram::runtime::push_redraw;

use super::{Ask, forget, press, run_answered, watch};
use crate::notify::test_host::{CHAT, TempRoot, TickHost};

fn keyboard() -> ReplyMarkup {
    ReplyMarkup::Inline(InlineKeyboardMarkup {
        inline_keyboard: Vec::new(),
    })
}

/// A switch is answered by the asked state reported by the current link: the same value carried
/// over a reconnect, or the other way round, keeps it waiting.
#[test]
fn a_switch_is_answered_by_its_state_from_the_current_link() {
    let state = CoreRunState {
        online: true,
        trading: Some(false),
        trading_confirmed: true,
        auto_detect: Some(true),
        started: Some(true),
        started_confirmed: false,
    };
    assert!(run_answered(state, RunSwitch::Trading, false));
    assert!(!run_answered(state, RunSwitch::Trading, true));
    // AutoDetect is confirmed by the runtime report, which this link has not sent yet.
    assert!(!run_answered(state, RunSwitch::AutoDetect, true));
    let confirmed = CoreRunState {
        started_confirmed: true,
        ..state
    };
    assert!(run_answered(confirmed, RunSwitch::AutoDetect, true));
    let carried = CoreRunState {
        trading_confirmed: false,
        ..state
    };
    assert!(!run_answered(carried, RunSwitch::Trading, false));
}

/// A press answered with a new message, or one that asked the core nothing, leaves no redraw.
#[test]
fn only_a_press_on_a_message_that_asked_the_core_waits() {
    let root = TempRoot::new("control-watch");
    let mut host = TickHost::open(root.notifications());
    let ask = Ask::CancelBuys { core: 3 };
    watch(&mut host, CHAT, None, ControlAction::Core(3), vec![ask]);
    watch(
        &mut host,
        CHAT,
        Some(10),
        ControlAction::Core(3),
        Vec::new(),
    );
    assert!(host.state.control_redraws.is_empty());
    watch(&mut host, CHAT, Some(10), ControlAction::Core(3), vec![ask]);
    assert!(host.state.control_redraws.contains_key(&(CHAT, 10)));
}

/// A press on a message withdraws its redraw, waiting or already queued, and nothing else.
#[test]
fn a_press_withdraws_its_messages_redraw_only() {
    let root = TempRoot::new("control-forget");
    let mut host = TickHost::open(root.notifications());
    let ask = Ask::CancelBuys { core: 3 };
    watch(&mut host, CHAT, Some(10), ControlAction::Core(3), vec![ask]);
    watch(&mut host, CHAT, Some(11), ControlAction::Core(3), vec![ask]);
    host.edit(|file| {
        push_redraw(file, CHAT, 10, "queued".into(), keyboard(), 1);
        push_redraw(file, CHAT, 11, "kept".into(), keyboard(), 1);
    });
    forget(&mut host, CHAT, 10);
    assert!(!host.state.control_redraws.contains_key(&(CHAT, 10)));
    assert!(host.state.control_redraws.contains_key(&(CHAT, 11)));
    assert_eq!(host.htmls(), vec!["kept".to_string()]);
}

/// The ⏳ button shows the very screen its redraw will draw: pressing it only looks again and
/// leaves the redraw waiting. Moving to another screen withdraws it.
#[test]
fn a_waiting_button_keeps_the_redraw_another_screen_drops_it() {
    let root = TempRoot::new("control-press");
    let mut host = TickHost::open(root.notifications());
    let ask = Ask::CancelBuys { core: 3 };
    watch(&mut host, CHAT, Some(10), ControlAction::Core(3), vec![ask]);
    assert!(press(&mut host, CHAT, 10, ControlAction::Core(3)).is_empty());
    assert!(host.state.control_redraws.contains_key(&(CHAT, 10)));
    assert!(
        press(
            &mut host,
            CHAT,
            10,
            ControlAction::Orders { core: 3, page: 0 }
        )
        .is_empty()
    );
    assert!(!host.state.control_redraws.contains_key(&(CHAT, 10)));
}
