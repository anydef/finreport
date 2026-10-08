//! `transactions` (§5): filter application, scoping, stable-order
//! pagination.

use entity::entities::transaction;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::graphql::scalars::{Date as GqlDate, Decimal as GqlDecimal, Uuid as GqlUuid};
use crate::graphql::types::{
    Direction, SortDirection, Transaction, TransactionFilter, TransactionPage, TransactionSort,
    TransactionSortField,
};

/// `PageInput.limit`, clamped to `1..=200` (§5). Pure and unit-tested.
pub fn clamp_limit(limit: i32) -> u64 {
    limit.clamp(1, 200) as u64
}

/// The filter's `amountMin`/`amountMax` as plain decimals. Both bound the
/// amount's **magnitude** (`abs(amount)`), inclusive.
pub(crate) fn amount_magnitude_bounds(
    filter: &TransactionFilter,
) -> (Option<rust_decimal::Decimal>, Option<rust_decimal::Decimal>) {
    (filter.amount_min.map(|d| d.0), filter.amount_max.map(|d| d.0))
}

/// The filter's `transactionIds`, if it names any. `Some(vec![])` is kept
/// distinct from `None`: an explicit empty list matches nothing, the same
/// as `categorySlugs` naming no known category.
pub(crate) fn transaction_id_list(filter: &TransactionFilter) -> Option<Vec<Uuid>> {
    filter
        .transaction_ids
        .as_ref()
        .map(|ids| ids.iter().map(|id| id.0).collect())
}

/// The filter's `counterpartyKeys`, if it names any. Unlike
/// `transactionIds`, an empty list is unconstrained (as `counterpartyNames`).
pub(crate) fn counterparty_key_list(filter: &TransactionFilter) -> Option<Vec<String>> {
    filter.counterparty_keys.clone().filter(|k| !k.is_empty())
}

/// Hand-rolled SQL twin of the `transactionIds`/`counterpartyKeys`/`amountMin`/`amountMax`
/// predicates in [`build_condition`], for the aggregation queries in
/// `cashflow` that do not go through sea-orm. `alias` is the transaction
/// table's alias (or name) in that query; `first_param` is the next free
/// positional parameter number. Returns the ` AND ...` fragment and the
/// parameters it consumed, in order.
pub(crate) fn id_and_amount_sql(
    alias: &str,
    filter: &TransactionFilter,
    first_param: usize,
) -> (String, Vec<sea_orm::Value>) {
    let mut sql = String::new();
    let mut params: Vec<sea_orm::Value> = Vec::new();
    let mut idx = first_param;
    if let Some(ids) = transaction_id_list(filter) {
        sql.push_str(&format!(" AND {alias}.id = ANY(${idx})"));
        params.push(ids.into());
        idx += 1;
    }
    if let Some(keys) = counterparty_key_list(filter) {
        sql.push_str(&format!(" AND {alias}.counterparty_key = ANY(${idx})"));
        params.push(keys.into());
        idx += 1;
    }
    let (min, max) = amount_magnitude_bounds(filter);
    if let Some(min) = min {
        sql.push_str(&format!(" AND ABS({alias}.amount) >= ${idx}"));
        params.push(min.into());
        idx += 1;
    }
    if let Some(max) = max {
        sql.push_str(&format!(" AND ABS({alias}.amount) <= ${idx}"));
        params.push(max.into());
    }
    (sql, params)
}

/// Builds the shared `WHERE` condition for both the plain transaction list
/// and (via `cashflow::build_summary_sql`'s hand-rolled SQL mirroring these
/// same predicates) the aggregation queries.
pub fn build_condition(scoped_ids: &[Uuid], filter: &TransactionFilter) -> Condition {
    let mut condition = Condition::all().add(transaction::Column::AccountId.is_in(scoped_ids.to_vec()));

    if let Some(ids) = transaction_id_list(filter) {
        condition = condition.add(transaction::Column::Id.is_in(ids));
    }
    if let Some(keys) = counterparty_key_list(filter) {
        condition = condition.add(transaction::Column::CounterpartyKey.is_in(keys));
    }
    let (amount_min, amount_max) = amount_magnitude_bounds(filter);
    if let Some(min) = amount_min {
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "ABS(transaction.amount) >= $1",
            vec![sea_orm::Value::from(min)],
        ));
    }
    if let Some(max) = amount_max {
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "ABS(transaction.amount) <= $1",
            vec![sea_orm::Value::from(max)],
        ));
    }
    if let Some(GqlDate(start)) = filter.start_date {
        condition = condition.add(transaction::Column::BookingDate.gte(start));
    }
    if let Some(GqlDate(end)) = filter.end_date {
        condition = condition.add(transaction::Column::BookingDate.lte(end));
    }
    if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
        // ILIKE, not LIKE: the SDL documents this field as a case-insensitive
        // substring, and the hand-rolled cashflow SQL twin
        // (`cashflow/summary.rs`, `cashflow/mod.rs`) has always used ILIKE —
        // so `LIKE` here made the charts and the transaction list disagree
        // about the same search term, with the list silently missing rows
        // whose casing differed.
        // Raw expression because sea-orm's `ColumnTrait` offers only `like`.
        let pattern = format!("%{search}%");
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "(transaction.counterparty_name ILIKE $1 OR transaction.description ILIKE $2)",
            vec![
                sea_orm::Value::from(pattern.clone()),
                sea_orm::Value::from(pattern),
            ],
        ));
    }
    match filter.direction {
        Some(Direction::Income) => {
            condition = condition.add(transaction::Column::Amount.gt(rust_decimal::Decimal::ZERO));
        }
        Some(Direction::Spending) => {
            condition = condition.add(transaction::Column::Amount.lt(rust_decimal::Decimal::ZERO));
        }
        None => {}
    }
    if let Some(names) = filter.counterparty_names.as_ref().filter(|n| !n.is_empty()) {
        condition = condition.add(transaction::Column::CounterpartyName.is_in(names.clone()));
    }
    match filter.has_counterparty {
        Some(true) => condition = condition.add(transaction::Column::CounterpartyName.is_not_null()),
        Some(false) => condition = condition.add(transaction::Column::CounterpartyName.is_null()),
        None => {}
    }
    if let Some(tags) = filter.tags.as_ref().filter(|t| !t.is_empty()) {
        // AND-ed (§4): the transaction must carry all of them.
        for tag in tags {
            condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
                "EXISTS (SELECT 1 FROM transaction_tag tt WHERE tt.transaction_id = transaction.id AND tt.tag = $1)",
                vec![sea_orm::Value::from(tag.clone())],
            ));
        }
    }
    if let Some(recurring) = filter.recurring {
        // Matches the *effective* flag (§2.2, §4): a user override on
        // `transaction_user_label.recurring` wins over the detector's
        // `transaction_insight.is_recurring`.
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "COALESCE(\
               (SELECT ul.recurring FROM transaction_user_label ul WHERE ul.transaction_id = transaction.id), \
               (SELECT ti.is_recurring FROM transaction_insight ti WHERE ti.transaction_id = transaction.id), \
               false\
             ) = $1",
            vec![sea_orm::Value::from(recurring)],
        ));
    }
    if let Some(transfer) = filter.transfer {
        // No override layer for transfers (§2.2): always the detector's
        // own flag, absent row meaning `false`.
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "COALESCE(\
               (SELECT ti.is_transfer FROM transaction_insight ti WHERE ti.transaction_id = transaction.id), \
               false\
             ) = $1",
            vec![sea_orm::Value::from(transfer)],
        ));
    }

    condition
}

/// Extends `build_condition` with the iteration-2 `categorySlugs`/
/// `uncategorized`/`needsReview`/`labelSources` filters (§5). Split into a
/// separate, `db`-touching function because resolving `categorySlugs`
/// (which must include descendants) needs a lookup against the `category`
/// table — the base `build_condition` stays synchronous/pure so
/// `cashflow`'s hand-rolled SQL can keep mirroring just its predicates.
pub async fn build_condition_with_category_filters(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
) -> async_graphql::Result<Condition> {
    let mut condition = build_condition(scoped_ids, filter);

    if let Some(slugs) = filter.category_slugs.as_ref().filter(|s| !s.is_empty()) {
        let ids = category_descendant_ids(db, slugs).await?;
        condition = condition.add(category_match_condition(&ids));
    }
    // `uncategorized` means **no category assigned**, which is what a user
    // means by it — not "no label row", which is what this used to test. A
    // transaction labelled while the taxonomy was empty has a row whose
    // `category_id` is NULL: it displays as "—" and is uncategorised in every
    // sense, but the old predicate excluded it, so filtering for
    // Uncategorized returned nothing while `breakdown.rs` (which buckets on
    // "has no category") reported a large Uncategorized total. Two
    // definitions of one word; this is now the breakdown's.
    //
    // A split transaction is *not* uncategorised: its categories live on the
    // parts, so `transaction_label.category_id` is legitimately NULL for it
    // (the labeler's split branch resolves to source=user with no category).
    // Valid splits therefore count as categorised, matching
    // `projection::labeling::find_valid_splits`' notion of a split that counts.
    //
    // `needsReview` stays separate: a held label and a missing category are
    // different states (iteration 2 spec §5).
    const HAS_CATEGORY: &str = "(EXISTS (SELECT 1 FROM transaction_label tl \
         WHERE tl.transaction_id = transaction.id AND tl.category_id IS NOT NULL) \
         OR EXISTS (SELECT 1 FROM transaction_split ts \
         WHERE ts.transaction_id = transaction.id AND ts.invalid = false))";
    match filter.uncategorized {
        Some(true) => {
            condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
                &format!("NOT {HAS_CATEGORY}"),
                Vec::<sea_orm::Value>::new(),
            ));
        }
        Some(false) => {
            condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
                HAS_CATEGORY,
                Vec::<sea_orm::Value>::new(),
            ));
        }
        None => {}
    }
    if filter.needs_review == Some(true) {
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.status = 'needs_review')",
            Vec::<sea_orm::Value>::new(),
        ));
    }
    if let Some(sources) = filter.label_sources.as_ref().filter(|s| !s.is_empty()) {
        let source_strs: Vec<String> = sources.iter().map(|s| gql_label_source_str(*s).to_string()).collect();
        condition = condition.add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.label_source = ANY($1))",
            vec![sea_orm::Value::from(source_strs)],
        ));
    }

    Ok(condition)
}

fn gql_label_source_str(source: crate::graphql::types::LabelSource) -> &'static str {
    match source {
        crate::graphql::types::LabelSource::User => "user",
        crate::graphql::types::LabelSource::Rule => "rule",
        crate::graphql::types::LabelSource::LlmCache => "llm-cache",
        crate::graphql::types::LabelSource::Llm => "llm",
    }
}

/// Every category id matching one of `slugs` or a descendant of one (§5:
/// "OR-ed; includes descendants"). The category table is small (≤ 3 levels)
/// so this loads it once and walks the parent chain in memory rather than
/// building a recursive SQL query.
async fn category_descendant_ids(
    db: &DatabaseConnection,
    slugs: &[String],
) -> async_graphql::Result<Vec<Uuid>> {
    let all = entity::entities::category::Entity::find().all(db).await?;
    let requested_ids: std::collections::HashSet<Uuid> = all
        .iter()
        .filter(|c| slugs.iter().any(|s| s == &c.slug))
        .map(|c| c.id)
        .collect();
    if requested_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(all
        .iter()
        .filter(|c| {
            let mut current = Some(c.id);
            let mut parent_of: std::collections::HashMap<Uuid, Option<Uuid>> = std::collections::HashMap::new();
            for row in &all {
                parent_of.insert(row.id, row.parent_id);
            }
            while let Some(id) = current {
                if requested_ids.contains(&id) {
                    return true;
                }
                current = parent_of.get(&id).copied().flatten();
            }
            false
        })
        .map(|c| c.id)
        .collect())
}

/// A transaction matches a category id set when either its whole-transaction
/// label resolves to one of them, or it has a valid split with a part in
/// one of them (§5's split-aware semantics, mirrored from
/// `breakdown.rs::fetch_breakdown`).
fn category_match_condition(ids: &[Uuid]) -> Condition {
    if ids.is_empty() {
        // No category in the tree matched any requested slug: the filter
        // can never match, same as an empty `IN ()`.
        return Condition::any().add(transaction::Column::Id.eq(Uuid::nil()));
    }
    Condition::any()
        .add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_label tl WHERE tl.transaction_id = transaction.id AND tl.category_id = ANY($1))",
            vec![sea_orm::Value::from(ids.to_vec())],
        ))
        .add(sea_orm::sea_query::Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM transaction_split ts WHERE ts.transaction_id = transaction.id AND ts.invalid = false AND ts.category_id = ANY($1))",
            vec![sea_orm::Value::from(ids.to_vec())],
        ))
}

/// Maps one `transaction` row onto the GraphQL type; shared with
/// `goals::goal_transactions` so both lists render identical rows.
pub(crate) fn to_graphql_transaction(row: transaction::Model) -> Transaction {
    Transaction {
        id: GqlUuid(row.id),
        account_id: GqlUuid(row.account_id),
        source: row.source,
        external_id: row.external_id,
        booking_date: GqlDate(row.booking_date),
        valuta_date: row.valuta_date.map(GqlDate),
        booking_status: row.booking_status,
        amount: GqlDecimal(row.amount),
        currency: row.currency,
        counterparty_name: row.counterparty_name,
        counterparty_iban: row.counterparty_iban,
        description: row.description,
        transaction_type: row.transaction_type,
        counterparty_key: row.counterparty_key,
    }
}

/// The resolved category's name, lower-cased for ordering: a user override
/// outranks the label (a tags-only override with no category falls through),
/// and a transaction with a valid split has no single category, so is NULL.
const CATEGORY_SORT_KEY: &str = "(CASE WHEN EXISTS (SELECT 1 FROM transaction_split ts \
       WHERE ts.transaction_id = transaction.id AND ts.invalid = false) THEN NULL \
     ELSE (SELECT LOWER(c.name) FROM category c WHERE c.id = COALESCE(\
       (SELECT ul.category_id FROM transaction_user_label ul WHERE ul.transaction_id = transaction.id), \
       (SELECT tl.category_id FROM transaction_label tl WHERE tl.transaction_id = transaction.id))) END)";

/// Orders a transaction select. `None` is today's order (newest booking
/// date first). Every ordering ends in the transaction id so equal keys
/// cannot swap between pages; NULL keys sort last in both directions.
pub(crate) fn apply_order(
    select: sea_orm::Select<transaction::Entity>,
    sort: Option<TransactionSort>,
) -> sea_orm::Select<transaction::Entity> {
    use sea_orm::sea_query::{Expr, NullOrdering};
    let Some(sort) = sort else {
        return select
            .order_by(transaction::Column::BookingDate, Order::Desc)
            .order_by(transaction::Column::ExternalId, Order::Desc)
            .order_by(transaction::Column::Id, Order::Desc);
    };
    let dir = match sort.direction {
        SortDirection::Asc => Order::Asc,
        SortDirection::Desc => Order::Desc,
    };
    let select = match sort.field {
        TransactionSortField::BookingDate => {
            // Date, then external id in the same direction, so DESC is
            // exactly the default order.
            return select
                .order_by(transaction::Column::BookingDate, dir.clone())
                .order_by(transaction::Column::ExternalId, dir.clone())
                .order_by(transaction::Column::Id, dir);
        }
        TransactionSortField::Amount => select.order_by(transaction::Column::Amount, dir),
        TransactionSortField::CounterpartyName => select.order_by_with_nulls(
            Expr::cust("LOWER(transaction.counterparty_name)"),
            dir,
            NullOrdering::Last,
        ),
        TransactionSortField::Category => {
            select.order_by_with_nulls(Expr::cust(CATEGORY_SORT_KEY), dir, NullOrdering::Last)
        }
    };
    select
        .order_by(transaction::Column::BookingDate, Order::Desc)
        .order_by(transaction::Column::Id, Order::Desc)
}

pub async fn fetch_transactions(
    db: &DatabaseConnection,
    scoped_ids: &[Uuid],
    filter: &TransactionFilter,
    limit: i32,
    offset: i32,
    sort: Option<TransactionSort>,
) -> async_graphql::Result<TransactionPage> {
    let limit = clamp_limit(limit);
    let offset = offset.max(0) as u64;

    if scoped_ids.is_empty() {
        return Ok(TransactionPage {
            items: vec![],
            total_count: 0,
            limit: limit as i32,
            offset: offset as i32,
        });
    }

    let condition = build_condition_with_category_filters(db, scoped_ids, filter).await?;
    let total_count = transaction::Entity::find()
        .filter(condition.clone())
        .count(db)
        .await?;

    let rows = apply_order(transaction::Entity::find().filter(condition), sort)
        .limit(limit)
        .offset(offset)
        .all(db)
        .await?;

    let items = rows.into_iter().map(to_graphql_transaction).collect();

    Ok(TransactionPage {
        items,
        total_count: total_count as i32,
        limit: limit as i32,
        offset: offset as i32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> GqlDecimal {
        GqlDecimal(s.parse().unwrap())
    }

    #[test]
    fn amount_bounds_pass_through_unsigned_and_independently() {
        let filter = TransactionFilter {
            amount_min: Some(dec("100")),
            ..Default::default()
        };
        assert_eq!(amount_magnitude_bounds(&filter), (Some(dec("100").0), None));
        assert_eq!(amount_magnitude_bounds(&TransactionFilter::default()), (None, None));
    }

    #[test]
    fn id_and_amount_sql_is_empty_for_an_unconstrained_filter() {
        let (sql, params) = id_and_amount_sql("t", &TransactionFilter::default(), 5);
        assert!(sql.is_empty());
        assert!(params.is_empty());
    }

    #[test]
    fn id_and_amount_sql_bounds_the_magnitude_and_numbers_params_from_the_start_index() {
        let filter = TransactionFilter {
            transaction_ids: Some(vec![GqlUuid(Uuid::nil())]),
            amount_min: Some(dec("10")),
            amount_max: Some(dec("20")),
            ..Default::default()
        };
        let (sql, params) = id_and_amount_sql("t", &filter, 7);
        assert_eq!(
            sql,
            " AND t.id = ANY($7) AND ABS(t.amount) >= $8 AND ABS(t.amount) <= $9"
        );
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn an_explicit_empty_id_list_stays_distinct_from_no_id_filter() {
        let none = TransactionFilter::default();
        let empty = TransactionFilter {
            transaction_ids: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(transaction_id_list(&none), None);
        assert_eq!(transaction_id_list(&empty), Some(vec![]));
        // ... and is ANDed in as `id = ANY('{}')`, i.e. matches nothing.
        let (sql, _) = id_and_amount_sql("transaction", &empty, 1);
        assert_eq!(sql, " AND transaction.id = ANY($1)");
    }

    #[test]
    fn id_and_amount_sql_only_amount_max_uses_the_first_param() {
        let filter = TransactionFilter {
            amount_max: Some(dec("5")),
            ..Default::default()
        };
        let (sql, params) = id_and_amount_sql("t", &filter, 3);
        assert_eq!(sql, " AND ABS(t.amount) <= $3");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn counterparty_keys_sql_sits_between_ids_and_amounts_and_empty_is_unconstrained() {
        let filter = TransactionFilter {
            transaction_ids: Some(vec![GqlUuid(Uuid::nil())]),
            counterparty_keys: Some(vec!["lidl".into(), "rewe".into()]),
            amount_max: Some(dec("20")),
            ..Default::default()
        };
        let (sql, params) = id_and_amount_sql("t", &filter, 1);
        assert_eq!(
            sql,
            " AND t.id = ANY($1) AND t.counterparty_key = ANY($2) AND ABS(t.amount) <= $3"
        );
        assert_eq!(params.len(), 3);

        let empty = TransactionFilter {
            counterparty_keys: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(counterparty_key_list(&empty), None);
        assert!(id_and_amount_sql("t", &empty, 1).0.is_empty());
    }

    #[test]
    fn counterparty_keys_condition_is_added_only_when_non_empty() {
        use sea_orm::{DbBackend, QueryTrait};
        let sql = |filter: &TransactionFilter| {
            transaction::Entity::find()
                .filter(build_condition(&[Uuid::nil()], filter))
                .build(DbBackend::Postgres)
                .to_string()
        };
        let with = sql(&TransactionFilter {
            counterparty_keys: Some(vec!["lidl".into(), "rewe".into()]),
            ..Default::default()
        });
        assert!(with.contains(r#""counterparty_key" IN ('lidl', 'rewe')"#), "{with}");
        let without = sql(&TransactionFilter {
            counterparty_keys: Some(vec![]),
            ..Default::default()
        });
        assert!(!without.contains(r#""counterparty_key" IN"#), "{without}");
    }

    fn order_sql(sort: Option<TransactionSort>) -> String {
        use sea_orm::{DbBackend, QueryTrait};
        let sql = apply_order(transaction::Entity::find(), sort)
            .build(DbBackend::Postgres)
            .to_string();
        sql[sql.find("ORDER BY").expect("ordered")..].to_string()
    }

    #[test]
    fn every_ordering_ends_in_the_transaction_id() {
        assert_eq!(
            order_sql(None),
            r#"ORDER BY "transaction"."booking_date" DESC, "transaction"."external_id" DESC, "transaction"."id" DESC"#
        );
        for field in [
            TransactionSortField::BookingDate,
            TransactionSortField::Amount,
            TransactionSortField::CounterpartyName,
            TransactionSortField::Category,
        ] {
            for direction in [SortDirection::Asc, SortDirection::Desc] {
                let sql = order_sql(Some(TransactionSort { field, direction }));
                assert!(sql.contains(r#""transaction"."id""#), "{field:?} {direction:?}: {sql}");
                assert!(sql.trim_end().ends_with("\"id\" ASC") || sql.trim_end().ends_with("\"id\" DESC"), "{sql}");
            }
        }
    }

    #[test]
    fn nullable_sort_keys_put_nulls_last_in_both_directions() {
        for field in [TransactionSortField::CounterpartyName, TransactionSortField::Category] {
            for direction in [SortDirection::Asc, SortDirection::Desc] {
                let sql = order_sql(Some(TransactionSort { field, direction }));
                assert!(sql.contains("NULLS LAST"), "{field:?} {direction:?}: {sql}");
            }
        }
    }

    #[test]
    fn limit_is_clamped_to_the_documented_range() {
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(-5), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(200), 200);
        assert_eq!(clamp_limit(201), 200);
        assert_eq!(clamp_limit(10_000), 200);
    }
}
