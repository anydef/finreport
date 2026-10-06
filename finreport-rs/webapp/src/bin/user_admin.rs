//! Admin CLI for creating users and granting account access (§4).
//!
//! ```text
//! user-admin create-user --username <u> [--display-name <n>]
//! user-admin set-password --username <u>
//! user-admin list-users
//! user-admin list-accounts
//! user-admin link --username <u> --account <source>:<external-id>|--all
//! user-admin unlink --username <u> --account <source>:<external-id>
//! ```
//!
//! Passwords come from `$FINREPORT_PASSWORD` or, failing that, a line read
//! from stdin — either way wrapped in a `SecretString` as soon as it is read,
//! never logged, never echoed by this binary.

use dotenv::dotenv;
use secrecy::SecretString;
use std::env;
use std::error::Error;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use tracing::error;
use tracing_subscriber::EnvFilter;
use utils::settings::Settings;
use webapp::auth;
use webapp::db::seaql::init_db;

const USAGE: &str = "\
usage:
  user-admin create-user --username <u> [--display-name <n>]
  user-admin set-password --username <u>
  user-admin list-users
  user-admin list-accounts
  user-admin link --username <u> (--account <source>:<external-id> | --all)
  user-admin unlink --username <u> --account <source>:<external-id>

password input: $FINREPORT_PASSWORD, or a line read from stdin if unset.";

#[derive(Debug)]
enum Command {
    CreateUser {
        username: String,
        display_name: Option<String>,
    },
    SetPassword {
        username: String,
    },
    ListUsers,
    ListAccounts,
    Link {
        username: String,
        account: AccountSelector,
    },
    Unlink {
        username: String,
        source: String,
        external_id: String,
    },
}

#[derive(Debug)]
enum AccountSelector {
    One { source: String, external_id: String },
    All,
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let command = match parse_command(&args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = run(command).await {
        error!(%e, "user-admin failed");
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(command: Command) -> Result<(), Box<dyn Error>> {
    let settings = Settings::from_env()?;
    let db = init_db(secrecy::ExposeSecret::expose_secret(
        settings.require_database_url()?,
    ))
    .await?;

    match command {
        Command::CreateUser {
            username,
            display_name,
        } => {
            let password = read_password()?;
            let user =
                auth::create_user(&db, &username, &password, display_name.as_deref()).await?;
            println!("created user {} ({})", user.username, user.id);
        }
        Command::SetPassword { username } => {
            let password = read_password()?;
            auth::set_password(&db, &username, &password).await?;
            println!("password updated for {username}");
        }
        Command::ListUsers => {
            let users = auth::list_users(&db).await?;
            if users.is_empty() {
                println!("no users");
            }
            for user in users {
                println!(
                    "{}\t{}\t{}{}",
                    user.id,
                    user.username,
                    user.display_name.as_deref().unwrap_or(""),
                    if user.disabled { "\t[disabled]" } else { "" }
                );
            }
        }
        Command::ListAccounts => {
            let accounts = auth::list_accounts(&db).await?;
            if accounts.is_empty() {
                println!("no accounts");
            }
            for account in accounts {
                println!(
                    "{}\t{}:{}\t{}",
                    account.id,
                    account.source,
                    account.external_id,
                    account.label.as_deref().unwrap_or("")
                );
            }
        }
        Command::Link { username, account } => match account {
            AccountSelector::One {
                source,
                external_id,
            } => {
                auth::link_account(&db, &username, &source, &external_id).await?;
                println!("linked {username} to {source}:{external_id}");
            }
            AccountSelector::All => {
                let linked = auth::link_all_accounts(&db, &username).await?;
                println!("linked {linked} account(s) to {username}");
            }
        },
        Command::Unlink {
            username,
            source,
            external_id,
        } => {
            auth::unlink_account(&db, &username, &source, &external_id).await?;
            println!("unlinked {username} from {source}:{external_id}");
        }
    }

    Ok(())
}

/// Reads the password for `create-user`/`set-password`: `$FINREPORT_PASSWORD`
/// if set, otherwise a single line from stdin. Wrapped in `SecretString`
/// immediately — the plain `String` it started as is dropped right here.
fn read_password() -> Result<SecretString, io::Error> {
    if let Ok(password) = env::var("FINREPORT_PASSWORD") {
        return Ok(SecretString::from(password));
    }

    eprint!("password: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let trimmed = line.trim_end_matches(['\n', '\r']);
    Ok(SecretString::from(trimmed.to_string()))
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    let (name, rest) = args
        .split_first()
        .ok_or_else(|| "missing subcommand".to_string())?;

    match name.as_str() {
        "create-user" => {
            let flags = parse_flags(rest, &["--username", "--display-name"])?;
            let username = require(&flags, "--username")?;
            let display_name = flags.get("--display-name").cloned();
            Ok(Command::CreateUser {
                username,
                display_name,
            })
        }
        "set-password" => {
            let flags = parse_flags(rest, &["--username"])?;
            let username = require(&flags, "--username")?;
            Ok(Command::SetPassword { username })
        }
        "list-users" => {
            parse_flags(rest, &[])?;
            Ok(Command::ListUsers)
        }
        "list-accounts" => {
            parse_flags(rest, &[])?;
            Ok(Command::ListAccounts)
        }
        "link" => {
            let flags = parse_flags(rest, &["--username", "--account"])?;
            let username = require(&flags, "--username")?;
            let account = match (flags.get("--account"), rest.iter().any(|a| a == "--all")) {
                (Some(spec), false) => {
                    let (source, external_id) = split_account(spec)?;
                    AccountSelector::One {
                        source,
                        external_id,
                    }
                }
                (None, true) => AccountSelector::All,
                (Some(_), true) => return Err("--account and --all are mutually exclusive".into()),
                (None, false) => {
                    return Err("link requires --account <source>:<external-id> or --all".into());
                }
            };
            Ok(Command::Link { username, account })
        }
        "unlink" => {
            let flags = parse_flags(rest, &["--username", "--account"])?;
            let username = require(&flags, "--username")?;
            let account = require(&flags, "--account")?;
            let (source, external_id) = split_account(&account)?;
            Ok(Command::Unlink {
                username,
                source,
                external_id,
            })
        }
        other => Err(format!("unknown subcommand {other:?}")),
    }
}

fn split_account(spec: &str) -> Result<(String, String), String> {
    spec.split_once(':')
        .map(|(source, external_id)| (source.to_string(), external_id.to_string()))
        .ok_or_else(|| format!("--account must be <source>:<external-id>, got {spec:?}"))
}

fn require(flags: &std::collections::HashMap<String, String>, key: &str) -> Result<String, String> {
    flags
        .get(key)
        .cloned()
        .ok_or_else(|| format!("{key} is required"))
}

/// Parses `--flag value` / `--flag=value` pairs out of `args`, rejecting
/// anything not in `known`. The standalone `--all` switch is skipped here and
/// handled by the `link` match arm directly.
fn parse_flags(
    args: &[String],
    known: &[&str],
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut flags = std::collections::HashMap::new();
    let mut iter = args.iter().peekable();

    while let Some(arg) = iter.next() {
        if arg == "--all" {
            continue;
        }
        if let Some((key, value)) = arg.split_once('=') {
            if !known.contains(&key) {
                return Err(format!("unexpected argument {key:?}"));
            }
            flags.insert(key.to_string(), value.to_string());
            continue;
        }
        if !known.contains(&arg.as_str()) {
            return Err(format!("unexpected argument {arg:?}"));
        }
        let value = iter
            .next()
            .ok_or_else(|| format!("{arg} requires a value"))?;
        flags.insert(arg.to_string(), value.to_string());
    }

    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_user_requires_username() {
        let err = parse_command(&["create-user".to_string()]).unwrap_err();
        assert!(err.contains("--username"));
    }

    #[test]
    fn create_user_parses_optional_display_name() {
        let args = [
            "create-user",
            "--username",
            "alice",
            "--display-name",
            "Alice A.",
        ]
        .map(String::from);
        match parse_command(&args).unwrap() {
            Command::CreateUser {
                username,
                display_name,
            } => {
                assert_eq!(username, "alice");
                assert_eq!(display_name.as_deref(), Some("Alice A."));
            }
            _ => panic!("expected CreateUser"),
        }
    }

    #[test]
    fn link_accepts_account_selector() {
        let args = ["link", "--username", "alice", "--account", "comdirect:123"].map(String::from);
        match parse_command(&args).unwrap() {
            Command::Link { username, account } => {
                assert_eq!(username, "alice");
                match account {
                    AccountSelector::One {
                        source,
                        external_id,
                    } => {
                        assert_eq!(source, "comdirect");
                        assert_eq!(external_id, "123");
                    }
                    AccountSelector::All => panic!("expected One"),
                }
            }
            _ => panic!("expected Link"),
        }
    }

    #[test]
    fn link_accepts_all_flag() {
        let args = ["link", "--username", "alice", "--all"].map(String::from);
        match parse_command(&args).unwrap() {
            Command::Link { account, .. } => assert!(matches!(account, AccountSelector::All)),
            _ => panic!("expected Link"),
        }
    }

    #[test]
    fn link_rejects_both_account_and_all() {
        let args = [
            "link",
            "--username",
            "alice",
            "--account",
            "comdirect:123",
            "--all",
        ]
        .map(String::from);
        assert!(parse_command(&args).is_err());
    }

    #[test]
    fn link_rejects_neither_account_nor_all() {
        let args = ["link", "--username", "alice"].map(String::from);
        assert!(parse_command(&args).is_err());
    }

    #[test]
    fn unlink_requires_account_selector() {
        let args = ["unlink", "--username", "alice"].map(String::from);
        assert!(parse_command(&args).is_err());
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(parse_command(&["bogus".to_string()]).is_err());
    }

    #[test]
    fn no_subcommand_is_rejected() {
        assert!(parse_command(&[]).is_err());
    }

    #[test]
    fn malformed_account_selector_is_rejected() {
        let args = ["unlink", "--username", "alice", "--account", "comdirect"].map(String::from);
        assert!(parse_command(&args).is_err());
    }
}
