//! `transactions(sort:)`: one test per sort field (order + nulls placement),
//! the default order, and a paging-stability proof for equal keys. Needs the
//! shared Postgres (`common::db`); gated like the rest of the suite.
#![cfg(feature = "integration")]

mod common;

use async_graphql::Request;
use chrono::Utc;
use entity::entities::transaction_user_label;
use sea_orm::{ActiveModelTrait, Set};
use std::collections::HashSet;
use uuid::Uuid;
use webapp::graphql::current_user::AuthenticatedUser;
use webapp::graphql::{create_schema, request_with_auth, RawSessionToken};

struct World {
    db: std::sync::Arc<sea_orm::DatabaseConnection>,
    user: AuthenticatedUser,
    account: Uuid,
    schema: webapp::graphql::AppSchema,
}

async fn world() -> World {
    let db = common::db().await;
    let (user_id, username) = common::seed_user(&db, "sort-test", "pw").await;
    let account = common::seed_account(&db, "EUR", "Sort account").await;
    common::link(&db, user_id, account).await;
    let schema = create_schema(db.clone(), common::dummy_settings());
    World {
        db,
        user: AuthenticatedUser { user_id, username, account_ids: vec![account] },
        account,
        schema,
    }
}

impl World {
    async fn tx(&self, date: &str, amount: &str, name: Option<&str>) -> Uuid {
        common::seed_transaction(&self.db, self.account, date, amount, name).await
    }

    /// Ids in the order the server returns them for `sort` (a GraphQL input
    /// literal, or empty for none) on one page.
    async fn ids(&self, sort: &str, limit: i32, offset: i32) -> Vec<Uuid> {
        let sort = if sort.is_empty() { String::new() } else { format!("sort: {sort},") };
        let q = format!(
            "{{ transactions({sort} page: {{limit: {limit}, offset: {offset}}}) {{ items {{ id }} }} }}"
        );
        let response = self
            .schema
            .execute(request_with_auth(Request::new(q), Some(self.user.clone()), RawSessionToken::default()))
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let json = response.data.into_json().unwrap();
        json["transactions"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().parse().unwrap())
            .collect()
    }
}

#[tokio::test]
async fn no_sort_keeps_newest_booking_date_first() {
    let w = world().await;
    let old = w.tx("2024-01-01", "1", None).await;
    let new = w.tx("2024-03-01", "1", None).await;
    let mid = w.tx("2024-02-01", "1", None).await;
    assert_eq!(w.ids("", 50, 0).await, vec![new, mid, old]);
    // An explicit BOOKING_DATE DESC (the input default) is the same order.
    assert_eq!(w.ids("{field: BOOKING_DATE}", 50, 0).await, vec![new, mid, old]);
    assert_eq!(w.ids("{field: BOOKING_DATE, direction: ASC}", 50, 0).await, vec![old, mid, new]);
}

#[tokio::test]
async fn amount_sorts_the_signed_value() {
    let w = world().await;
    let big_out = w.tx("2024-01-01", "-500.00", None).await;
    let small_out = w.tx("2024-01-01", "-5.00", None).await;
    let small_in = w.tx("2024-01-01", "20.00", None).await;
    let big_in = w.tx("2024-01-01", "300.00", None).await;
    assert_eq!(
        w.ids("{field: AMOUNT, direction: DESC}", 50, 0).await,
        vec![big_in, small_in, small_out, big_out]
    );
    assert_eq!(
        w.ids("{field: AMOUNT, direction: ASC}", 50, 0).await,
        vec![big_out, small_out, small_in, big_in]
    );
}

#[tokio::test]
async fn counterparty_name_is_case_insensitive_and_nulls_sort_last_both_ways() {
    let w = world().await;
    let b = w.tx("2024-01-01", "1", Some("bravo")).await;
    let a = w.tx("2024-01-01", "1", Some("Alpha")).await;
    let c = w.tx("2024-01-01", "1", Some("Charlie")).await;
    let none = w.tx("2024-01-01", "1", None).await;
    assert_eq!(w.ids("{field: COUNTERPARTY_NAME, direction: ASC}", 50, 0).await, vec![a, b, c, none]);
    assert_eq!(w.ids("{field: COUNTERPARTY_NAME, direction: DESC}", 50, 0).await, vec![c, b, a, none]);
}

#[tokio::test]
async fn category_sorts_by_resolved_name_override_wins_and_uncategorised_is_last() {
    let w = world().await;
    let (apples, _) = common::seed_category(&w.db, "sort_a", "Apples", "expense").await;
    let (pears, _) = common::seed_category(&w.db, "sort_p", "pears", "expense").await;
    let (zucchini, _) = common::seed_category(&w.db, "sort_z", "Zucchini", "expense").await;

    let t_apples = w.tx("2024-01-01", "1", None).await;
    common::seed_transaction_label(&w.db, t_apples, Some(apples), "rule", "applied").await;
    let t_pears = w.tx("2024-01-01", "1", None).await;
    common::seed_transaction_label(&w.db, t_pears, Some(pears), "llm", "applied").await;
    // Labelled Apples, but the user pinned Zucchini: the override wins.
    let t_override = w.tx("2024-01-01", "1", None).await;
    common::seed_transaction_label(&w.db, t_override, Some(apples), "rule", "applied").await;
    transaction_user_label::ActiveModel {
        transaction_id: Set(t_override),
        category_id: Set(Some(zucchini)),
        note: Set(None),
        revision: Set(Utc::now().into()),
        recurring: Set(None),
    }
    .insert(w.db.as_ref())
    .await
    .unwrap();
    // A label row with no category, and a transaction with no label at all.
    let t_null_cat = w.tx("2024-01-01", "1", None).await;
    common::seed_transaction_label(&w.db, t_null_cat, None, "llm", "needs_review").await;
    let t_unlabelled = w.tx("2024-01-02", "1", None).await;

    let asc = w.ids("{field: CATEGORY, direction: ASC}", 50, 0).await;
    assert_eq!(&asc[..3], &[t_apples, t_pears, t_override]);
    let tail: HashSet<_> = asc[3..].iter().copied().collect();
    assert_eq!(tail, HashSet::from([t_null_cat, t_unlabelled]));

    let desc = w.ids("{field: CATEGORY, direction: DESC}", 50, 0).await;
    assert_eq!(&desc[..3], &[t_override, t_pears, t_apples]);
    // Nulls stay last when DESC; within them, newest booking date first.
    assert_eq!(&desc[3..], &[t_unlabelled, t_null_cat]);
}

#[tokio::test]
async fn equal_keys_page_without_repeating_or_dropping_rows() {
    let w = world().await;
    // 12 rows identical in every sortable key.
    let mut all: HashSet<Uuid> = HashSet::new();
    for _ in 0..12 {
        all.insert(w.tx("2024-05-05", "10.00", Some("Same")).await);
    }
    for sort in [
        "{field: AMOUNT}",
        "{field: COUNTERPARTY_NAME, direction: ASC}",
        "{field: CATEGORY}",
        "{field: BOOKING_DATE, direction: ASC}",
    ] {
        let mut seen: Vec<Uuid> = Vec::new();
        for offset in (0..12).step_by(5) {
            seen.extend(w.ids(sort, 5, offset).await);
        }
        let unique: HashSet<_> = seen.iter().copied().collect();
        assert_eq!(seen.len(), 12, "{sort}: a row was dropped or repeated");
        assert_eq!(unique, all, "{sort}: pages did not cover exactly the set");
        // Re-reading page 1 gives the same rows: the order is deterministic.
        assert_eq!(w.ids(sort, 5, 0).await, seen[..5].to_vec(), "{sort}");
    }
}
