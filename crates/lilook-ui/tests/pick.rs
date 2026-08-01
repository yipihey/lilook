//! The list every picker in lilook is built from: what it offers for what has
//! been typed, and where a click on it lands.

use lilook_ui::pick;

/// Typing narrows by word, not only by first letter.
///
/// The value is what someone has in mind -- `smooth`, `viridis` -- and which
/// parameter carries it is the thing they are asking the list for. Matching only
/// the front of the label would mean having to know the answer first.
#[test]
fn what_is_typed_narrows_by_any_word_of_the_offer() {
    assert!(pick::matches("interpolation", "int"));
    assert!(pick::matches("interpolation: \"smooth\"", "smo"));
    assert!(pick::matches("map: viridis", "VIRI"), "case is not a rule");
    assert!(pick::matches("anything", ""), "nothing typed, everything");
    assert!(!pick::matches("interpolation", "oo"), "not a substring");

    let items = ["mark", "mark-size", "stroke"];
    let got: Vec<&&str> = pick::matching(&items, "mark", |s| s);
    assert_eq!(got, vec![&"mark", &"mark-size"]);
}

/// The picture is part of the target.
///
/// This is the property the colormap menu did not have: `selectable_label` senses
/// only the text it painted, so the ramp -- the widest thing in the row, and the
/// only part of it that says what the colour map actually looks like -- was
/// inert. Clicking it did nothing at all. So the click here lands on the ramp,
/// well to the left of the name, and is asserted to select the row.
#[test]
fn a_click_on_the_picture_selects_the_row() {
    let ctx = egui::Context::default();
    let id = egui::Id::new("row");
    let clicks = std::cell::Cell::new(0);
    let rect = std::cell::Cell::new(egui::Rect::NOTHING);

    let frame = |input: egui::RawInput| {
        let _ = ctx.run_ui(input, |ui| {
            let r = pick::click_row(ui, id, false, |ui| {
                pick::ramp(ui, "viridis");
                ui.label("viridis");
            });
            rect.set(r.rect);
            if r.clicked() {
                clicks.set(clicks.get() + 1);
            }
        });
    };

    // Let the row settle: a first pass has no font metrics to lay out with.
    for _ in 0..3 {
        frame(egui::RawInput::default());
    }
    assert_eq!(clicks.get(), 0, "drawing a row is not choosing it");

    // Over the ramp: the left-hand end of the row, nowhere near the text. Then
    // over the name itself, which a selectable label would otherwise swallow.
    let row = rect.get();
    let click_at = |x: f32| {
        let at = egui::pos2(x, row.center().y);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(at));
        for pressed in [true, false] {
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            });
        }
        input
    };
    frame(click_at(row.left() + 6.0));
    assert_eq!(clicks.get(), 1, "the ramp is part of the row, not scenery");
    frame(click_at(row.left() + 60.0));
    assert_eq!(clicks.get(), 2, "and so is the name beside it");
}

/// A cycle's swatches are its whole palette.
///
/// The array's own brackets used to reach the colour parser, so the first and
/// last entry of every literal palette failed to parse and Okabe-Ito showed six
/// of its eight colours -- a preview that quietly misrepresents what it previews.
#[test]
fn a_palette_previews_every_colour_in_it() {
    for (name, expr, _) in lilook_core::CYCLES {
        let colors = pick::cycle_colors(expr);
        let wanted = expr.matches("rgb(").count() + expr.matches("luma(").count();
        assert_eq!(colors.len(), wanted, "{name} previews {colors:?}");
        assert!(!colors.is_empty(), "{name} previews nothing");
    }
}
