//! Quick fixes: what lilook offers to do about a broken figure.
//!
//! Driven by `(message, document)` plus `blame`, because most of lilaq's errors
//! carry no span. The test that matters for each one is the same: apply the
//! offer, and the figure compiles.

use lilook_compile::{backend::Hints, blame, Backend};
use lilook_core::{Document, Schema, Session};

const SCHEMA: &str = lilook_core::schema::BUNDLED;

/// Compile, collect blame for anything spanless, and ask for actions.
fn offers(src: &str) -> Option<(Session, Vec<lilook_core::Action>)> {
    let mut b = Backend::new(std::env::temp_dir(), "");
    let mut session = Session::new(src, Schema::from_json(SCHEMA).expect("schema"));
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if render
        .errors()
        .any(|d| d.message.contains("package") || d.message.contains("network"))
    {
        eprintln!("lilaq unavailable; skipping");
        return None;
    }
    assert!(render.failed(), "the fixture is supposed to be broken");
    session.diagnostics = render.diagnostics.clone();
    session.scenes = scenes;
    let mut blames = vec![];
    for d in render.errors().filter(|d| d.range.is_none()) {
        blames.extend(blame::locate(&mut b, &doc, &d.message));
    }
    let actions = session.actions(&blames);
    Some((session, actions))
}

/// Every offer, applied, produces a figure that compiles.
///
/// The catalogue is drawn from failures this project actually hit while being
/// built, which is why each one is worth offering.
#[test]
fn every_offered_fix_produces_a_figure_that_compiles() {
    const HEAD: &str = "#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 5pt)\n";
    let cases = [
        (
            "log axis with a negative limit",
            format!("{HEAD}#lq.diagram(yscale: \"log\", ylim: (-1, 100), lq.plot((1, 2, 3), (1, 10, 100)))\n"),
        ),
        (
            "an empty limit array",
            format!("{HEAD}#lq.diagram(xlim: (), lq.plot((1, 2, 3), (1, 2, 3)))\n"),
        ),
        (
            "a misspelled parameter",
            format!("{HEAD}#lq.diagram(widht: 8cm, lq.plot((1, 2, 3), (1, 2, 3)))\n"),
        ),
    ];

    let mut b = Backend::new(std::env::temp_dir(), "");
    for (label, src) in cases {
        let Some((session, actions)) = offers(&src) else {
            return;
        };
        assert!(!actions.is_empty(), "{label}: nothing offered");

        for action in &actions {
            let mut trial = Session::new(&src, session.schema.clone());
            trial.scenes = session.scenes.clone();
            trial.apply_action(action);
            assert_ne!(trial.doc.text(), src, "{label}/{}: no edit", action.label);

            let r = b.render(trial.doc.text(), 1.0);
            assert!(
                !r.failed(),
                "{label}: applying {:?} left it broken: {:?}",
                action.label,
                r.errors().next().map(|d| d.message.clone())
            );

            // One undoable step, and it undoes.
            trial.doc.undo();
            assert_eq!(
                trial.doc.text(),
                src,
                "{label}/{}: did not undo",
                action.label
            );
        }
    }
}

/// A misspelling is corrected to the nearest real parameter, keeping its value.
#[test]
fn a_misspelled_parameter_is_offered_the_name_that_was_meant() {
    let src = "#import \"@preview/lilaq:0.6.0\" as lq\n#set page(width: auto, height: auto, margin: 5pt)\n#lq.diagram(widht: 8cm, lq.plot((1, 2, 3), (1, 2, 3)))\n";
    let Some((_, actions)) = offers(src) else {
        return;
    };
    let rename = actions
        .iter()
        .find(|a| a.label.contains("rename"))
        .unwrap_or_else(|| panic!("no rename offered: {actions:?}"));
    assert!(
        rename.label.contains("`widht`") && rename.label.contains("`width`"),
        "{}",
        rename.label
    );
    // Two intents, because a rename is a removal and an insertion -- and the
    // value travels with it.
    assert_eq!(rename.intents.len(), 2);
}

/// A healthy figure is offered nothing.
#[test]
fn nothing_is_offered_for_a_figure_that_works() {
    let src = "#import \"@preview/lilaq:0.6.0\" as lq\n#lq.diagram(lq.plot((1, 2), (1, 2)))\n";
    let mut b = Backend::new(std::env::temp_dir(), "");
    let doc = Document::new(src);
    let mut hints = Hints::new();
    let (render, scenes) = b.render_scenes(&doc, 1.0, &mut hints);
    if render.failed() {
        return;
    }
    let mut s = Session::new(src, Schema::from_json(SCHEMA).expect("schema"));
    s.diagnostics = render.diagnostics.clone();
    s.scenes = scenes;
    assert!(s.actions(&[]).is_empty());
}
