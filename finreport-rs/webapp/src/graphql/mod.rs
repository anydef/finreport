mod mutations;
mod queries;
pub mod scalars;
pub mod types;

use crate::graphql::mutations::MutationRoot;
use crate::graphql::queries::QueryRoot;
use async_graphql::{EmptySubscription, Schema};
use sea_orm::DatabaseConnection;

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

pub fn create_schema(conn: DatabaseConnection) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        // `DateTime` (§5) isn't reachable from any field yet — nothing in
        // this iteration's stub resolvers returns a raw timestamp — so it
        // needs an explicit registration or the exporter would silently drop
        // it from the SDL.
        .register_output_type::<scalars::DateTime>()
        .data(conn)
        .finish()
}
