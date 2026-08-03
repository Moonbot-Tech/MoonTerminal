use super::RowSelection;

#[test]
fn a_press_selects_exactly_one_row() {
    let mut sel = RowSelection::default();
    sel.press(4);

    assert_eq!(sel.range(), Some(4..5));
    assert!(sel.contains(4));
    assert!(!sel.contains(5));
    assert!(sel.drag_to(5), "нажатие оставляет перетаскивание активным");
}

#[test]
fn dragging_upwards_selects_the_rows_in_between() {
    let mut sel = RowSelection::default();
    sel.press(7);
    assert!(sel.drag_to(3), "новая строка под курсором — есть изменение");

    assert_eq!(sel.range(), Some(3..8), "порядок концов не важен");
}

#[test]
fn dragging_onto_the_same_row_changes_nothing() {
    let mut sel = RowSelection::default();
    sel.press(2);

    assert!(!sel.drag_to(2), "тот же индекс — перерисовывать нечего");
}

#[test]
fn a_move_after_release_does_not_extend() {
    let mut sel = RowSelection::default();
    sel.press(2);
    sel.release();

    assert!(
        !sel.drag_to(9),
        "кнопка отпущена — движение мыши не выделяет"
    );
    assert_eq!(sel.range(), Some(2..3));
}

#[test]
fn shift_press_extends_from_the_existing_anchor() {
    let mut sel = RowSelection::default();
    sel.press(2);
    sel.release();
    sel.shift_press(6);

    assert_eq!(sel.range(), Some(2..7));
}

#[test]
fn shift_press_without_an_anchor_starts_a_selection() {
    let mut sel = RowSelection::default();
    sel.shift_press(5);

    assert_eq!(sel.range(), Some(5..6));
}

#[test]
fn a_shrunken_list_drops_the_selection() {
    let mut sel = RowSelection::default();
    sel.press(9);
    sel.release();

    sel.clamp_to(10);
    assert_eq!(sel.range(), Some(9..10), "строка ещё существует");

    sel.clamp_to(9);
    assert!(sel.is_empty(), "строки больше нет — выделение снято");
}

#[test]
fn an_empty_selection_has_no_range() {
    let sel = RowSelection::default();

    assert!(sel.is_empty());
    assert_eq!(sel.range(), None);
    assert!(!sel.contains(0));
}

#[test]
fn select_all_takes_every_row_and_ends_the_gesture() {
    let mut sel = RowSelection::default();
    sel.select_all(4);

    assert_eq!(sel.range(), Some(0..4));
    assert!(
        !sel.drag_to(9),
        "выделение всего не оставляет активный драг"
    );
}

#[test]
fn select_all_of_an_empty_list_selects_nothing() {
    let mut sel = RowSelection::default();
    sel.press(0);
    sel.select_all(0);

    assert!(sel.is_empty());
}

#[test]
fn a_copy_command_outside_the_selection_takes_only_its_own_row() {
    let mut sel = RowSelection::default();
    sel.press(2);
    sel.drag_to(4);

    assert_eq!(
        sel.range_for(3),
        2..5,
        "строка внутри выделения — берём всё"
    );
    assert_eq!(sel.range_for(9), 9..10, "строка вне выделения — только она");
}
