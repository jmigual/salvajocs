//! Wine prefixes of the Heroic, Lutris and Bottles launchers, and the title normalization used
//! to match a launcher's game to a manifest entry.

/// Symbols that separate a title's parts; they become spaces.
const GAP_SYMBOLS: &[char] = &['™', '®', '©', ':', '-'];

/// Quotes that are dropped from a title.
const NO_GAP_SYMBOLS: &[char] = &['\'', '"', '‘', '’', '“', '”'];

/// Normalizes a game title for matching, after ludusavi's `normalize_title`: lowercase, no
/// trailing ` (<year>)`, no `… edition` suffix, `™®©:-` as spaces, no quotes, single spaces.
/// Unlike ludusavi's, the year only matches ASCII digits.
pub fn normalize_title(title: &str) -> String {
    let mut title = title.to_lowercase();
    strip_year(&mut title);
    strip_punctuated_edition(&mut title);
    if let Some(stripped) = title.strip_suffix(" game of the year edition") {
        title.truncate(stripped.len());
    }
    strip_short_edition(&mut title);
    let title: String = title
        .chars()
        .filter(|c| !NO_GAP_SYMBOLS.contains(c))
        .map(|c| if GAP_SYMBOLS.contains(&c) { ' ' } else { c })
        .collect();
    title
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

/// Drops a trailing ` (<digits>)`.
fn strip_year(title: &mut String) {
    let Some(inner) = title.strip_suffix(')') else {
        return;
    };
    if let Some(start) = inner.rfind(" (")
        && let digits = &inner[start + 2..]
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
    {
        title.truncate(start);
    }
}

/// Drops `<symbol> <words> edition` from the first `™®©:-` that starts one.
fn strip_punctuated_edition(title: &mut String) {
    let Some(body) = title.strip_suffix(" edition") else {
        return;
    };
    let cut = body.char_indices().find(|&(i, c)| {
        let rest = &body[i + c.len_utf8()..];
        GAP_SYMBOLS.contains(&c) && rest.len() > 1 && rest.starts_with(' ')
    });
    if let Some((i, _)) = cut {
        title.truncate(i);
    }
}

/// Drops a one-word ` <word> edition`; a longer edition name may be part of the title.
fn strip_short_edition(title: &mut String) {
    let Some(body) = title.strip_suffix(" edition") else {
        return;
    };
    if let Some(space) = body.rfind(' ')
        && space + 1 < body.len()
    {
        title.truncate(space);
    }
}

#[cfg(test)]
mod tests;
