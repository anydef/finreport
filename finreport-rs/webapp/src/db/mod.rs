use categorizer::categorize::{CategorizeAiResponse, Category};

use sqlx::{Connection, Row, SqliteConnection};
use std::error::Error;
use comdirect_rs::comdirect::transaction::Transaction;

pub const DB_URL: &str = "sqlite://sqlite.db";

pub struct Persistence {
    db: SqliteConnection,
}


impl Persistence {
    pub async fn new(db_path: &str) -> Result<Self, Box<dyn Error>> {
        let db = SqliteConnection::connect(db_path)
            .await
            .expect("Failed to open database");
        let mut persistence = Self { db };
        Self::init_transactions(&mut persistence.db)
            .await
            .expect("Failed to initialize transactions");
        Self::init_categories(&mut persistence.db)
            .await
            .expect("Failed to initialize categories");
        Ok(persistence)
    }
    pub async fn init_transactions(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS transactions (
            reference TEXT PRIMARY KEY,
            account_id TEXT,
            booking_status TEXT NOT NULL,
            booking_date TEXT NOT NULL,
            amount REAL NOT NULL,
            remitter TEXT,
            deptor TEXT,
            creditor TEXT,
            creditor_id TEXT,
            creditor_mandate_id TEXT,
            remittance_info TEXT,
            transaction_type TEXT
        )"#,
        )
        .execute(conn)
        .await?;

        Ok(())
    }
    pub async fn init_categories(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS categories (
            id INTEGER PRIMARY KEY,
            category TEXT,
            subcategory TEXT)"#,
        )
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS transaction_categories (
            reference TEXT,
            category_id INTEGER,
            reasoning TEXT,
            confidence REAL,
            FOREIGN KEY (reference) REFERENCES transactions(reference),
            FOREIGN key (category_id) REFERENCES categories(id),
            PRIMARY KEY (reference, category_id)
        )"#,
        )
        .execute(&mut *conn)
        .await?;

        Ok(())
    }

    pub async fn load_categories(&mut self, categories: &[Category]) -> Result<(), sqlx::Error> {
        for category in categories {
            for subcategory in &category.subcategories {
                sqlx::query(
                    r#"INSERT OR REPLACE INTO categories (category, subcategory)
                    VALUES (?, ?)"#,
                )
                .bind(&category.category)
                .bind(&subcategory)
                .execute(&mut self.db)
                .await?;
            }
        }
        Ok(())
    }

    pub async fn insert_transactions(
        &mut self,
        transactions: &[Transaction],
    ) -> Result<(), sqlx::Error> {
        // Use a transaction for better performance with multiple inserts
        let mut tx = self.db.begin().await?;

        for transaction in transactions {
            let empty_string = String::new();
            sqlx::query(
                r#"INSERT OR REPLACE INTO transactions
                (reference, booking_status, booking_date, amount, remitter,
                 deptor, creditor, creditor_id, creditor_mandate_id, remittance_info, transaction_type)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
            )
                .bind(&transaction.reference)
                .bind(&transaction.booking_status)
                .bind(&transaction.booking_date)
                .bind(&transaction.amount.value)
                .bind(&transaction.remitter.as_ref().map(|r| &r.holder_name).unwrap_or(&empty_string))
                .bind(&transaction.deptor.as_ref().unwrap_or(&empty_string))
                .bind(&transaction.creditor.as_ref().map(|c| &c.holder_name).unwrap_or(&empty_string))
                .bind(&transaction.direct_debit_creditor_id)
                .bind(&transaction.direct_debit_creditor_id)
                .bind(&transaction.remittance_info)
                .bind(&transaction.transaction_type.key)
                .execute(&mut *tx)
                .await?;
        }

        // Commit the transaction
        tx.commit().await?;

        Ok(())
    }
    pub async fn add_category(
        &mut self,
        t: &Transaction,
        c: &CategorizeAiResponse,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.db.begin().await?;
        // Insert or update the category
        sqlx::query(
            r#"INSERT OR REPLACE INTO categories (category, subcategory)
            VALUES (?, ?)"#,
        )
        .bind(&c.category)
        .bind("category")
        .execute(&mut *tx)
        .await
        .expect("Failed to insert or update category");

        let result =
            sqlx::query(r#"SELECT id from categories WHERE category = ? AND subcategory = ?"#)
                .bind(&c.category)
                .bind(&c.subcategory)
                .fetch_one(&mut *tx)
                .await
                .expect("Failed to fetch category id");

        sqlx::query(
            r#"INSERT OR REPLACE INTO transaction_categories (reference, category_id, reasoning, confidence)
            VALUES (?, ?, ?, ?)"#,
        )
            .bind(&t.reference)
            .bind(result.get::<i64, _>(0))
            .bind(&c.reasoning)
            .bind(&c.confidence)
            .execute(&mut *tx)
            .await.expect("Failed to insert transaction category");

        tx.commit().await.expect("Failed to commit transaction");
        Ok(())
    }

    pub async fn check_categorized(
        &mut self,
        transaction: &Transaction,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.db.begin().await?;

        let row = sqlx::query(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM transaction_categories
                WHERE reference = ?
            ) AS is_categorized
        "#,
        )
        .bind(&transaction.reference)
        .fetch_one(&mut *tx)
        .await?;

        let is_categorized: bool = row.get(0);

        Ok(is_categorized)
    }
}
