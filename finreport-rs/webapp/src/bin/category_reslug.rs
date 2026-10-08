//! One-off repair for categories created with a bare leaf slug and a parent
//! (see `webapp::category_reslug` for the design and the order of operations).
//!
//! ```text
//! category-reslug [--dry-run] [--only <current-slug>]...
//! ```
//!
//! Idempotent: a second run finds nothing broken and changes nothing.

use std::error::Error;

use dotenv::dotenv;
use sea_orm::Database;
use secrecy::ExposeSecret;
use tracing::error;
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::category_reslug::{Options, run};
use webapp::kafka::producer::EventPublisher;

const USAGE: &str = "[--dry-run] [--only <current-slug>]...";

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Options, String> {
    let mut opts = Options::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dry-run" => opts.dry_run = true,
            "--only" => opts.only.push(args.next().ok_or("--only needs a slug")?),
            other => return Err(format!("unknown argument {other:?}; usage: {USAGE}")),
        }
    }
    Ok(opts)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let opts = parse_args(std::env::args().skip(1)).map_err(|e| {
        error!(%e, "[startup] invalid arguments");
        e
    })?;

    let settings = Settings::from_env()?;
    let brokers = settings.require_kafka_brokers()?.to_string();
    let conn = Database::connect(settings.require_database_url()?.expose_secret()).await?;
    let publisher = if opts.dry_run {
        None
    } else {
        Some(EventPublisher::connect(&brokers)?)
    };

    run(&conn, &brokers, publisher.as_ref(), &opts)
        .await
        .map_err(|e| -> Box<dyn Error> { e.to_string().into() })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        let o = parse(&["--dry-run", "--only", "gas", "--only", "hobby"]).unwrap();
        assert!(o.dry_run);
        assert_eq!(o.only, vec!["gas", "hobby"]);
        assert!(!parse(&[]).unwrap().dry_run);
    }

    #[test]
    fn rejects_unknown_and_dangling() {
        assert!(parse(&["--wat"]).is_err());
        assert!(parse(&["--only"]).is_err());
    }
}
