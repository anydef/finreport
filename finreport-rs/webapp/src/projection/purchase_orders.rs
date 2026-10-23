//! Projection of `finreport.purchase-order` into `purchase_order` +
//! `purchase_order_item`.
//!
//! Revision-guarded last-writer-wins like the other per-user topics: a record
//! older than the stored one is a no-op, so a replay or an out-of-order echo
//! never regresses a newer upload. Items are replaced wholesale with their
//! order inside the batch transaction `projection::process_batch` already
//! holds, so a reader never sees an order with half its items. A tombstone
//! deletes the order and its items.
//!
//! Only fields of [`amazon_orders::Order`] are written. That type has no
//! recipient fields, so the projection cannot store an address even if a
//! record carried one (serde drops unknown fields on deserialise).

use amazon_orders::PriceBasis;
use entity::entities::{purchase_order, purchase_order_item};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue::Set, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::kafka::purchase_orders::PurchaseOrderRecord;

fn price_basis_str(basis: PriceBasis) -> &'static str {
    match basis {
        PriceBasis::Indistinguishable => "indistinguishable",
        PriceBasis::UnitPrice => "unit_price",
        PriceBasis::LineTotal => "line_total",
        PriceBasis::Unreconciled => "unreconciled",
    }
}

/// Upserts (`Some`) or deletes (`None`) the order at `(user_id, source,
/// order_id)` - the topic key, which is authoritative for a tombstone.
pub async fn project_purchase_order(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    source: &str,
    order_id: &str,
    record: Option<PurchaseOrderRecord>,
) -> Result<(), DbErr> {
    let Some(record) = record else {
        delete_items(txn, user_id, source, order_id).await?;
        purchase_order::Entity::delete_by_id((user_id, source.to_string(), order_id.to_string()))
            .exec(txn)
            .await?;
        return Ok(());
    };

    if let Some(existing) =
        purchase_order::Entity::find_by_id((user_id, source.to_string(), order_id.to_string()))
            .one(txn)
            .await?
        && existing.revision > record.revision
    {
        return Ok(());
    }

    let order = &record.order;
    let model = purchase_order::ActiveModel {
        user_id: Set(user_id),
        source: Set(source.to_string()),
        order_id: Set(order_id.to_string()),
        order_date: Set(order.order_date),
        total: Set(order.total),
        currency: Set(order.currency.clone()),
        price_basis: Set(price_basis_str(order.price_basis).to_string()),
        discounts_netted: Set(order.discounts_netted),
        remainder: Set(order.remainder),
        revision: Set(record.revision.into()),
    };
    purchase_order::Entity::insert(model)
        .on_conflict(
            OnConflict::columns([
                purchase_order::Column::UserId,
                purchase_order::Column::Source,
                purchase_order::Column::OrderId,
            ])
            .update_columns([
                purchase_order::Column::OrderDate,
                purchase_order::Column::Total,
                purchase_order::Column::Currency,
                purchase_order::Column::PriceBasis,
                purchase_order::Column::DiscountsNetted,
                purchase_order::Column::Remainder,
                purchase_order::Column::Revision,
            ])
            .to_owned(),
        )
        .exec(txn)
        .await?;

    delete_items(txn, user_id, source, order_id).await?;
    let items: Vec<purchase_order_item::ActiveModel> = order
        .items
        .iter()
        .enumerate()
        .map(|(position, item)| purchase_order_item::ActiveModel {
            user_id: Set(user_id),
            source: Set(source.to_string()),
            order_id: Set(order_id.to_string()),
            position: Set(position as i32),
            asin: Set(item.asin.clone()),
            quantity: Set(item.quantity as i32),
            price: Set(item.price),
            discount: Set(item.discount),
            promotion: Set(item.promotion.clone()),
            promotion_amount: Set(item.promotion_amount),
            title: Set(item.title.clone()),
            status: Set(item.status.clone()),
            returned: Set(item.returned),
            line_total: Set(item.line_total),
        })
        .collect();
    if !items.is_empty() {
        purchase_order_item::Entity::insert_many(items).exec(txn).await?;
    }
    Ok(())
}

async fn delete_items(
    txn: &impl ConnectionTrait,
    user_id: Uuid,
    source: &str,
    order_id: &str,
) -> Result<(), DbErr> {
    purchase_order_item::Entity::delete_many()
        .filter(purchase_order_item::Column::UserId.eq(user_id))
        .filter(purchase_order_item::Column::Source.eq(source))
        .filter(purchase_order_item::Column::OrderId.eq(order_id))
        .exec(txn)
        .await?;
    Ok(())
}
