//! Argument parsing shared by the binaries that drive a single Comdirect login.

/// Reads `--account <key>` (or `--account=<key>`) from the process arguments.
///
/// The importer imports every configured account by default; this flag narrows
/// a run to a single one, which is mostly useful locally. `init-session`, which
/// drives one login interactively, requires it when several are configured.
pub fn account_arg() -> Result<Option<String>, String> {
    parse_account(std::env::args().skip(1))
}

fn parse_account(args: impl Iterator<Item = String>) -> Result<Option<String>, String> {
    let mut args = args.peekable();
    let mut account = None;

    while let Some(arg) = args.next() {
        match arg.split_once('=') {
            Some(("--account", key)) => account = Some(key.to_string()),
            _ if arg == "--account" => {
                account = Some(
                    args.next()
                        .ok_or_else(|| "--account requires a value".to_string())?,
                );
            }
            _ => return Err(format!("unexpected argument {arg:?}; usage: [--account <key>]")),
        }
    }

    Ok(account)
}

#[cfg(test)]
mod test {
    use super::parse_account;

    fn parse(args: &[&str]) -> Result<Option<String>, String> {
        parse_account(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn account_is_optional() {
        assert_eq!(parse(&[]).unwrap(), None);
    }

    #[test]
    fn both_flag_spellings_are_accepted() {
        assert_eq!(parse(&["--account", "1"]).unwrap().as_deref(), Some("1"));
        assert_eq!(parse(&["--account=joint"]).unwrap().as_deref(), Some("joint"));
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse(&["--account"]).is_err());
        assert!(parse(&["1"]).is_err());
    }
}
