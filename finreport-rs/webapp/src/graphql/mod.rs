mod current_user;
mod mutations;
mod queries;

use crate::graphql::mutations::MutationRoot;
use crate::graphql::queries::QueryRoot;
use async_graphql::{EmptySubscription, Schema};
use sea_orm::DatabaseConnection;

pub type AppSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

pub fn create_schema(conn: DatabaseConnection) -> AppSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(conn)
        .finish()
}
