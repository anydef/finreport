use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};

pub struct QueryRoot;

#[derive(SimpleObject)]
pub struct Report {
    pub month: String,
    pub year: String,
    pub category: String,
    pub total_income: f64,
    pub total_expenses: f64,
}

#[Object]
impl QueryRoot {
    async fn hello(&self) -> &str {
        "Hello, world!"
    }

    async fn reports(&self, month: String, year: String) -> Vec<Report> {
        dbg!(&month);
        vec![
            Report {
                month: month.clone(),
                year: year.clone(),
                category: "Groceries".to_string(),
                total_income: 1000.0,
                total_expenses: 500.0,
            },
            Report {
                month: month.clone(),
                year: year.clone(),
                category: "Utilities".to_string(),
                total_income: 1000.0,
                total_expenses: 500.0,
            },
            Report {
                month: month.clone(),
                year: year.clone(),
                category: "Entertainment".to_string(),
                total_income: 1000.0,
                total_expenses: 500.0,
            },
        ]
    }
}

pub type AppSchema = Schema<QueryRoot, EmptyMutation, EmptySubscription>;

pub fn create_schema() -> AppSchema {
    Schema::build(QueryRoot, EmptyMutation, EmptySubscription).finish()
}
