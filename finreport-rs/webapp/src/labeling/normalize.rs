//! §2.5 normalization: "lowercase, strip accents, collapse whitespace, drop a
//! trailing legal form (`gmbh`, `ag`, `e.k.`, `kg`, `se`), drop card-terminal
//! noise (`//`, `sagt danke`, trailing store numbers, `kartenzahlung`, a
//! trailing date), falling back to the normalized description when the
//! counterparty is empty."
//!
//! WP0 stub — owned by WP2. The projector (WP3) writes this function's
//! result to `transaction.counterparty_key` (§3); the fake provider (WP0's
//! `categorizer::provider::fake`) and real providers (WP1) consume its
//! output via `LabelRequest::counterparty`, they never call it themselves.

/// Trailing "legal form" tokens stripped from the end of a counterparty
/// name, plus the connector tokens (`&`, `co`, `co.`) that can sit between
/// two of them (e.g. "Aldi GmbH & Co. KG" → "aldi"). Compared after
/// lowercasing and with any trailing `.` removed from the token, so `e.k.`
/// and `ek` both match.
const TRAILING_LEGAL_FORM_TOKENS: &[&str] = &["gmbh", "ag", "ek", "kg", "se", "co", "&"];

/// Normalizes a counterparty (or, if empty, a description) into the stable
/// key rules, the LLM fingerprint and the learner index on.
pub fn normalize(counterparty: Option<&str>, description: Option<&str>) -> String {
    let source = counterparty
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| description.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");

    let lowered = source.to_lowercase();
    let folded = strip_accents(&lowered);
    let without_noise = strip_card_terminal_noise(&folded);
    let without_legal_form = strip_trailing_legal_form(&without_noise);
    collapse_whitespace(&without_legal_form)
}

/// Folds common Latin-1/German diacritics to their closest ASCII letter.
/// `ß` becomes `ss` (its standard ASCII expansion), everything else maps
/// one-to-one. Characters outside this table pass through unchanged.
fn strip_accents(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            'ä' => out.push('a'),
            'ö' => out.push('o'),
            'ü' => out.push('u'),
            'ß' => out.push_str("ss"),
            'á' | 'à' | 'â' | 'ã' | 'å' => out.push('a'),
            'é' | 'è' | 'ê' | 'ë' => out.push('e'),
            'í' | 'ì' | 'î' | 'ï' => out.push('i'),
            'ó' | 'ò' | 'ô' | 'õ' => out.push('o'),
            'ú' | 'ù' | 'û' => out.push('u'),
            'ñ' => out.push('n'),
            'ç' => out.push('c'),
            other => out.push(other),
        }
    }
    out
}

/// Repeatedly strips a trailing legal-form or connector token (whitespace
/// delimited, any `.`/`,` ignored for comparison, so `e.k.` matches `ek`)
/// until the last remaining token is not one of
/// [`TRAILING_LEGAL_FORM_TOKENS`].
fn strip_trailing_legal_form(input: &str) -> String {
    let mut tokens: Vec<&str> = input.split_whitespace().collect();
    while let Some(last) = tokens.last() {
        let bare: String = last.chars().filter(|c| *c != '.' && *c != ',').collect();
        if TRAILING_LEGAL_FORM_TOKENS.contains(&bare.as_str()) {
            tokens.pop();
        } else {
            break;
        }
    }
    tokens.join(" ")
}

/// Drops card-terminal noise: a `//` separator and anything after it (card
/// terminals use it to append terminal/location metadata), the phrase
/// "sagt danke", the literal "kartenzahlung", a trailing store-number token
/// and a trailing date token (`dd.mm`, `dd.mm.yy` or `dd.mm.yyyy`, also
/// accepting `/` or `-` as the separator).
fn strip_card_terminal_noise(input: &str) -> String {
    let before_terminal_meta = input.split("//").next().unwrap_or(input);
    let without_phrases = before_terminal_meta
        .replace("sagt danke", " ")
        .replace("kartenzahlung", " ");

    let mut tokens: Vec<&str> = without_phrases.split_whitespace().collect();
    while let Some(last) = tokens.last() {
        if is_trailing_date_token(last) || is_store_number_token(last) {
            tokens.pop();
        } else {
            break;
        }
    }
    tokens.join(" ")
}

/// A standalone run of ASCII digits (2+ of them) — a trailing store/terminal
/// number, not a meaningful word.
fn is_store_number_token(token: &str) -> bool {
    token.len() >= 2 && token.chars().all(|c| c.is_ascii_digit())
}

/// `dd.mm`, `dd.mm.yy` or `dd.mm.yyyy`, separator `.`, `/` or `-`.
fn is_trailing_date_token(token: &str) -> bool {
    let parts: Vec<&str> = token.split(['.', '/', '-']).collect();
    if parts.len() != 2 && parts.len() != 3 {
        return false;
    }
    if !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    let day: u32 = match parts[0].parse() {
        Ok(d) => d,
        Err(_) => return false,
    };
    let month: u32 = match parts[1].parse() {
        Ok(m) => m,
        Err(_) => return false,
    };
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return false;
    }
    if parts.len() == 3 {
        let year_len = parts[2].len();
        if year_len != 2 && year_len != 4 {
            return false;
        }
    }
    true
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_and_collapses_whitespace() {
        assert_eq!(normalize(Some("  REWE   Markt "), None), "rewe markt");
    }

    #[test]
    fn strips_common_accents() {
        assert_eq!(normalize(Some("Käfer Bäckerei"), None), "kafer backerei");
        assert_eq!(normalize(Some("Straße"), None), "strasse");
        assert_eq!(normalize(Some("Café René"), None), "cafe rene");
    }

    #[test]
    fn strips_trailing_legal_forms() {
        let cases = [
            ("Rewe GmbH", "rewe"),
            ("Aldi GmbH & Co. KG", "aldi"),
            ("Edeka AG", "edeka"),
            ("Fielmann SE", "fielmann"),
            ("Mustermann e.K.", "mustermann"),
            ("Handwerk KG", "handwerk"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize(Some(input), None), expected, "input: {input}");
        }
    }

    #[test]
    fn legal_form_token_mid_string_is_kept() {
        // "ag" inside a word, or as a non-trailing token, is not a legal
        // form and must survive.
        assert_eq!(normalize(Some("AG Coffee Roasters"), None), "ag coffee roasters");
    }

    #[test]
    fn strips_card_terminal_noise() {
        let cases = [
            ("Lidl // Filiale 123", "lidl"),
            ("Rewe sagt danke 12345", "rewe"),
            ("Edeka Kartenzahlung", "edeka"),
            ("Netto 01.02", "netto"),
            ("Netto 01.02.2024", "netto"),
            ("Netto 01/02/24", "netto"),
            ("Aldi 42", "aldi"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize(Some(input), None), expected, "input: {input}");
        }
    }

    #[test]
    fn combines_legal_form_and_noise_stripping() {
        assert_eq!(
            normalize(Some("Rewe GmbH sagt danke // Filiale 9"), None),
            "rewe"
        );
    }

    #[test]
    fn falls_back_to_description_when_counterparty_empty() {
        assert_eq!(normalize(None, Some("Dauerauftrag Miete")), "dauerauftrag miete");
        assert_eq!(normalize(Some(""), Some("Dauerauftrag Miete")), "dauerauftrag miete");
        assert_eq!(normalize(Some("   "), Some("Miete")), "miete");
    }

    #[test]
    fn both_empty_yields_empty_string() {
        assert_eq!(normalize(None, None), "");
        assert_eq!(normalize(Some(""), Some("")), "");
    }

    #[test]
    fn description_is_also_normalized() {
        assert_eq!(normalize(None, Some("  Überweisung Gehalt  ")), "uberweisung gehalt");
    }

    #[test]
    fn non_date_trailing_number_pair_is_not_mistaken_for_a_date() {
        // "13.45" has an invalid month, so it is not stripped as a date —
        // but it is still an all-digit-with-separator *store* token only
        // when each segment is itself a bare digit run; this exercises that
        // "13.45" is rejected as a date while remaining otherwise untouched.
        assert_eq!(normalize(Some("Shop 13.45"), None), "shop 13.45");
    }
}
