//! The tape block's status line, and the Russian copy the user approved.

use super::{TapeFacts, TapeStatus, tape_status};

/// Swapping the priority would name a job over dashes that have no window, or tell the user
/// an unsaved draft is the thing to do while the write button is already disabled.
#[test]
fn station_tape_status_picks_one_line() {
    let line = |known, dirty, busy, older_service| {
        tape_status(TapeFacts {
            known,
            dirty,
            busy,
            older_service,
        })
    };
    assert_eq!(line(true, false, false, false), TapeStatus::Clean);
    assert_eq!(line(true, true, false, false), TapeStatus::Draft);
    assert_eq!(line(true, true, true, false), TapeStatus::Busy);
    assert_eq!(line(false, true, true, false), TapeStatus::Down);
    assert_eq!(line(false, false, false, true), TapeStatus::Older);
    // A window that was read stays clean even if a stale older-service flag is still set.
    assert_eq!(line(true, false, false, true), TapeStatus::Clean);
}

/// Rewriting the approved Russian, or dropping a placeholder, puts the old "Set" button
/// or a raw `%{margin}` back in front of the user.
#[test]
fn station_tape_russian_copy_is_the_approved_wording() {
    let _locale = crate::test_locale::force("ru");
    let text = |key: &str| rust_i18n::t!(key).to_string();
    assert_eq!(
        text("telegram.server.tape_hint"),
        "Станция круглосуточно записывает трейды рынка вокруг каждой сделки — даже когда терминал выключен. По этим записям тюнер потом проверяет варианты входа и выхода. Здесь задаётся, сколько записывать."
    );
    assert_eq!(
        text("telegram.server.tape_margin_hint"),
        "Сколько секунд рынка записать до входа и после выхода. Меньше 30 с нельзя — столько нужно тюнеру, чтобы судить выход."
    );
    assert_eq!(
        text("telegram.server.tape_long_hint"),
        "Сделка дольше этого пишется только по краям: вокруг входа и вокруг выхода. Середина не пишется — меньше места на диске, но тюнер не судит варианты, которые вышли бы в середине."
    );
    assert_eq!(text("telegram.server.tape_set"), "Записать на станцию");
    assert_eq!(
        text("telegram.server.tape_clean"),
        "Станция пишет с этими значениями."
    );
    assert_eq!(
        rust_i18n::t!(
            "telegram.server.tape_draft",
            margin = "30 с",
            long = "5 мин"
        )
        .as_ref(),
        "Не сохранено: сейчас станция пишет 30 с и 5 мин."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.tape_busy", job = "Обновить службу").as_ref(),
        "Идёт «Обновить службу» — кнопки вернутся, когда она закончится."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.tape_down", status = "Статус").as_ref(),
        "Станция не отвечает — её значения неизвестны. Проверьте, что она запущена («Статус»)."
    );
    assert_eq!(
        text("telegram.server.tape_footnote"),
        "У терминала свои такие же настройки для его собственной записи (Настройки → Хранилище → «Трейды сделок»). Станция взяла их один раз при установке, дальше меняется только здесь: к одной станции могут быть подключены несколько терминалов, и ни один не перепишет её молча."
    );
    assert_eq!(
        rust_i18n::t!("telegram.server.running", job = "Статус").as_ref(),
        "Выполняется: Статус…"
    );
}
