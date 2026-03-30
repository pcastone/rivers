/// Dispatch exec subcommands.
pub fn cmd_exec(args: &[String]) -> Result<(), String> {
    match args[0].as_str() {
        "hash" => {
            if args.len() < 2 {
                return Err("Usage: riversctl exec hash <path>".into());
            }
            cmd_exec_hash(&args[1])
        }
        "verify" => {
            if args.len() < 3 {
                return Err("Usage: riversctl exec verify <path> <sha256>".into());
            }
            cmd_exec_verify(&args[1], &args[2])
        }
        "list" => {
            eprintln!("exec list: not yet implemented — use bundle TOML to review declared commands");
            Err("exec list is planned for a future release".into())
        }
        other => Err(format!("unknown exec subcommand: '{other}'\nUsage: riversctl exec <hash|verify|list> [args]")),
    }
}

/// Compute SHA-256 of a file and print in TOML-ready format.
fn cmd_exec_hash(path: &str) -> Result<(), String> {
    use sha2::{Sha256, Digest};
    let bytes = std::fs::read(path).map_err(|e| {
        format!("cannot read {path}: {e}")
    })?;
    let hash = Sha256::digest(&bytes);
    println!("sha256 = \"{}\"", hex::encode(hash));
    Ok(())
}

/// Verify a file matches an expected SHA-256 hash.
fn cmd_exec_verify(path: &str, expected: &str) -> Result<(), String> {
    use sha2::{Sha256, Digest};

    // Validate expected hash format
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid sha256: expected 64 hex characters, got '{expected}'"));
    }

    let bytes = std::fs::read(path).map_err(|e| {
        format!("cannot read {path}: {e}")
    })?;
    let actual = hex::encode(Sha256::digest(&bytes));

    if actual == expected {
        println!("  [OK]    {path}: hash matches");
        Ok(())
    } else {
        println!("  [FAIL]  {path}: expected {}, got {}", &expected[..16], &actual[..16]);
        Err(format!("hash mismatch for {path}"))
    }
}
