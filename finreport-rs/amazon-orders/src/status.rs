//! Loose, language-tolerant reading of the free-text `Status` column.

/// Lower-case stems that mean "returned / refunded" in the languages Amazon
/// storefronts use. A substring match, never an exact one, so
/// "Return started", "Returned", "Rückgabe eingeleitet" and "Erstattet" all hit.
const RETURN_STEMS: &[&str] = &[
    "return",    // en
    "refund",    // en
    "rückgabe",  // de
    "rueckgabe", // de, transliterated
    "retoure",   // de
    "erstattet", // de
    "erstattung", // de
    "retour",    // fr/nl
    "remboursé", // fr
    "rembours",  // fr
    "devol",     // es/pt
    "reembols",  // es
    "rimbors",   // it
    "restitu",   // it/es
];

/// Phrases that contain a return stem but describe a *delivered, kept* item
/// ("Return window closed", "Return or replace items").
const NOT_A_RETURN: &[&str] = &["window", "eligible", "or replace", "fenster", "frist"];

/// True when the status text looks like the item is being / was returned or
/// refunded. Never panics on unfamiliar text; unknown means `false`.
pub fn looks_returned(status: &str) -> bool {
    let s = status.to_lowercase();
    if NOT_A_RETURN.iter().any(|p| s.contains(p)) {
        return false;
    }
    RETURN_STEMS.iter().any(|stem| s.contains(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_statuses() {
        let cases = [
            ("Return started", true),
            ("Returned", true),
            ("Refunded", true),
            ("Rückgabe eingeleitet", true),
            ("Erstattet", true),
            ("Retour gestart", true),
            ("Delivered 7 October", false),
            ("Arriving today", false),
            ("Zugestellt am 7. Oktober", false),
            ("Return window closed", false),
            ("", false),
        ];
        for (status, expected) in cases {
            assert_eq!(looks_returned(status), expected, "status {status:?}");
        }
    }
}
