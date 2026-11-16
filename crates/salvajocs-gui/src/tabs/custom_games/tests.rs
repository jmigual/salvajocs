use super::*;
use salvajocs_core::manifest::GameEntry;

fn game(name: &str) -> CustomGame {
    CustomGame {
        name: name.into(),
        entry: GameEntry::default(),
        integration: Integration::Extend,
        wine_prefix: None,
        pairs: vec![Pair {
            linux: "<xdgData>/Foo".into(),
            windows: "<winAppData>/Foo".into(),
        }],
    }
}

#[test]
fn a_game_changed_since_the_form_opened_is_not_overwritten() {
    let mut games = vec![game("Game")];
    let form = Form::edit(0, &games[0]);
    assert_eq!(form.build(&games), Ok(games[0].clone()));

    games[0].pairs.push(Pair {
        linux: "<xdgData>/Bar".into(),
        windows: "<winAppData>/Bar".into(),
    });
    assert_eq!(form.build(&games), Err(FormError::Changed));
    games.clear();
    assert_eq!(form.build(&games), Err(FormError::Changed));
}

#[cfg(unix)]
#[test]
fn an_untouched_wine_prefix_keeps_its_exact_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut stored = game("Game");
    stored.wine_prefix = Some(PathBuf::from(OsStr::from_bytes(b"/prefixes/caf\xe9")));
    let mut form = Form::edit(0, &stored);
    assert_eq!(form.build(&[stored.clone()]), Ok(stored.clone()));

    form.wine_prefix = "/prefixes/other".into();
    let built = form.build(&[stored]).unwrap();
    assert_eq!(built.wine_prefix, Some(PathBuf::from("/prefixes/other")));
}
