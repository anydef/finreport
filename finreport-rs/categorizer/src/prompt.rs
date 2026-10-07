//! Renders the §2.9 system prompt (`prompts/categorize.txt`) for a given
//! [`LabelRequest`]: fills in the prompt-schema version and the flattened
//! slug catalog, then appends the concrete transaction as the user turn.
//!
//! Shared by every real provider so a prompt change only has to be made
//! once; `fake` never calls this (it is deterministic by keyword, not by
//! LLM).

use crate::provider::{CategoryCatalog, LabelRequest};

/// The committed prompt template, rewritten for the slug catalog and the
/// structured `category_slug` / `proposed_path` / `confidence` / `ambiguous`
/// response (§2.9). Read at compile time so a template edit always ships
/// with the binary that reads it.
const TEMPLATE: &str = include_str!("../../../prompts/categorize.txt");

/// Builds the system prompt: the template with `{prompt_version}` and
/// `{catalog}` substituted.
pub fn system_prompt(catalog: &CategoryCatalog, prompt_version: &str) -> String {
    let catalog_lines = render_catalog(catalog);
    TEMPLATE
        .replace("{prompt_version}", prompt_version)
        .replace("{catalog}", &catalog_lines)
}

/// Builds the user turn: the concrete transaction fields a provider needs to
/// categorize, one per line so the model does not have to parse a nested
/// structure.
pub fn user_turn(req: &LabelRequest<'_>) -> String {
    let counterparty = req.counterparty.unwrap_or("(none)");
    let description = req.description.unwrap_or("(none)");
    let transaction_type = req.transaction_type.unwrap_or("(unknown)");
    format!(
        "counterparty: {counterparty}\n\
         description: {description}\n\
         amount: {amount} {currency}\n\
         booking_date: {booking_date}\n\
         transaction_type: {transaction_type}",
        amount = req.amount,
        currency = req.currency,
        booking_date = req.booking_date,
    )
}

/// `slug — name — kind`, one entry per line, in catalog order.
fn render_catalog(catalog: &CategoryCatalog) -> String {
    catalog
        .entries
        .iter()
        .map(|entry| format!("- {} — {} — {}", entry.slug, entry.name, entry.kind))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CatalogEntry;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;

    fn catalog() -> CategoryCatalog {
        CategoryCatalog::new(vec![CatalogEntry {
            slug: "food.groceries".to_string(),
            name: "Groceries".to_string(),
            kind: "expense".to_string(),
        }])
    }

    #[test]
    fn system_prompt_substitutes_version_and_catalog() {
        let prompt = system_prompt(&catalog(), "7");

        assert!(prompt.contains("prompt schema version 7"));
        assert!(prompt.contains("- food.groceries — Groceries — expense"));
        assert!(!prompt.contains("{prompt_version}"));
        assert!(!prompt.contains("{catalog}"));
    }

    #[test]
    fn user_turn_falls_back_for_missing_optional_fields() {
        let catalog = catalog();
        let req = LabelRequest {
            counterparty: None,
            description: Some("REWE SAGT DANKE"),
            amount: Decimal::new(-1250, 2),
            currency: "EUR",
            booking_date: NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            transaction_type: None,
            catalog: &catalog,
            prompt_version: "2",
        };

        let turn = user_turn(&req);

        assert!(turn.contains("counterparty: (none)"));
        assert!(turn.contains("description: REWE SAGT DANKE"));
        assert!(turn.contains("transaction_type: (unknown)"));
    }
}
