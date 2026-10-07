//! Compatibility shim, **not** part of the §2.9 provider rewrite.
//!
//! `webapp/src/db/mod.rs` (the legacy sqlite `Persistence`, frozen per the
//! iteration-2 shared-file protocol) is the one remaining caller of these two
//! plain data structs. The ad-hoc `dotenv`/`env_logger`/`config` settings
//! loader that used to live alongside them is gone — iteration-2 providers
//! read `utils::settings::Settings` instead (§2.9) — but deleting the
//! structs too would break that frozen file for a reason unrelated to this
//! work package, so they stay here, trimmed to exactly what it uses.
use serde::Deserialize;
use std::fmt::{Display, Formatter};

#[derive(Deserialize, Debug, Clone)]
pub struct Category {
    pub category: String,
    pub subcategories: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CategorizeAiResponse {
    pub reference: String,
    pub category: String,
    pub subcategory: String,
    pub confidence: f32,
    pub reasoning: String,
}

impl Display for CategorizeAiResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Reference: {}, Category: {}, Subcategory: {}, Confidence: {}, Reasoning: {}",
            self.reference, self.category, self.subcategory, self.confidence, self.reasoning
        )
    }
}
