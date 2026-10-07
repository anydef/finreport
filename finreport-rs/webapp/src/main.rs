use actix_web::web;
use actix_web::{App, HttpResponse, HttpServer};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use dotenv::dotenv;
use secrecy::ExposeSecret;
use std::sync::Arc;
use tracing::error;
use utils::settings::Settings;
use webapp::auth::bootstrap_admin;
use webapp::db::seaql;
use webapp::graphql::{create_schema, http::cors, http::graphql_resource};

async fn playground() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let app_settings =
        Arc::new(Settings::from_env().expect("Could not load application settings"));

    let database_url = app_settings
        .require_database_url()
        .expect("APP_database_url is required to run webapp");
    let conn = Arc::new(
        seaql::init_db(database_url.expose_secret())
            .await
            .expect("Failed to connect to the database"),
    );

    if let Err(e) = bootstrap_admin(
        &conn,
        &app_settings.admin_username,
        app_settings.admin_password.as_ref(),
    )
    .await
    {
        // Never fatal: a broken bootstrap must not take an otherwise-healthy
        // GraphQL server down (the existing `app_user` row, if any, still
        // logs in fine).
        error!(%e, "[startup] admin bootstrap failed");
    }

    let schema = create_schema(Arc::clone(&conn), Arc::clone(&app_settings));
    let cors_settings = Arc::clone(&app_settings);
    let handler_settings = Arc::clone(&app_settings);
    HttpServer::new(move || {
        let app = App::new()
            .wrap(cors(&cors_settings))
            .app_data(web::Data::new(schema.clone()))
            .app_data(web::Data::new(Arc::clone(&conn)))
            .app_data(web::Data::new(Arc::clone(&handler_settings)))
            .service(graphql_resource());

        // `/playground` is unauthenticated and has no business being
        // reachable from a release build — only wired up in debug builds
        // (`cargo build`/`cargo run` without `--release`), never in the
        // images actually deployed.
        if cfg!(debug_assertions) {
            app.route("/playground", web::get().to(playground))
        } else {
            app
        }
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
