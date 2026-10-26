//! Local admin commands — the break-glass path (design doc §5). They act on
//! the data dir directly, so they work when the web UI can't be reached or the
//! IdP is down. Run as the service user on the host:
//!
//! ```text
//! bifrost config example                   (a commented $BIFROST_DATA/config.toml)
//! bifrost auth status
//! bifrost auth mode native|forward-auth     (restart the service to apply)
//! bifrost admin reset-link <username>
//! bifrost admin create-sysadmin <username>  (prints a reset link to set the password)
//! bifrost admin set-role <username> user|moderator|sysadmin
//! ```

use crate::auth::{self, AuthInner, AuthMode, ForwardAuth, Role, Throttle};
use crate::db::Db;

const USAGE: &str = "usage:
  bifrost                                   run the server
  bifrost config example                    print a commented config.toml template
  bifrost auth status                       show the auth mode and accounts
  bifrost auth mode native|forward-auth     switch auth mode (restart to apply)
  bifrost admin reset-link <username>       mint a 24 h password-reset link
  bifrost admin create-sysadmin <username>  create a sysadmin + reset link
  bifrost admin set-role <username> <role>  user|moderator|sysadmin (e.g. promote
                                            the first IdP user when no group mapping)
  bifrost backup now                        take a backup (also runs nightly)
  bifrost backup list                       list backups
  bifrost backup restore <file.zip>         restore one (stop Bifrost first)
  bifrost season import <bundle.tar.gz>     add a finished season from another
                                            instance's archive bundle";

/// Commands that need no data dir or database (run before startup touches
/// either). Returns the exit code if `args` named one.
pub fn run_early(args: &[String]) -> Option<i32> {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["config", "example"] => {
            print!("{}", crate::config::EXAMPLE);
            Some(0)
        }
        _ => None,
    }
}

/// Commands that need the data dir but must run before the database is
/// opened (restoring replaces it).
pub fn run_offline(args: &[String]) -> Option<i32> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["backup", "restore", file] => Some(match crate::backup::restore(std::path::Path::new(file)) {
            Ok(msg) => {
                println!("{msg}");
                0
            }
            Err(e) => {
                eprintln!("restore failed: {e}");
                1
            }
        }),
        _ => None,
    }
}

/// Run a CLI command if `args` (after the program name) name one. Returns the
/// process exit code, or `None` to start the server.
pub async fn run(args: &[String], db: &Db) -> Option<i32> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match args.as_slice() {
        [] => return None,
        ["auth", "status"] => status(db).await,
        ["auth", "mode", mode] => set_mode(db, mode).await,
        ["admin", "reset-link", user] => reset_link(db, user).await,
        ["admin", "create-sysadmin", user] => create_sysadmin(db, user).await,
        ["admin", "set-role", user, role] => set_role(db, user, role).await,
        ["backup", "now"] => match crate::backup::create(db, &crate::backup::Policy::from_config()).await {
            Ok(b) => {
                println!("{}", crate::backup::dir().join(&b.name).display());
                0
            }
            Err(e) => {
                eprintln!("backup failed: {e}");
                1
            }
        },
        ["season", "import", file] => match crate::archive::import(db, std::path::Path::new(file), None).await {
            Ok(id) => {
                println!("imported as season {id} (archived)");
                0
            }
            Err(e) => {
                eprintln!("import failed: {e}");
                1
            }
        },
        ["backup", "list"] => {
            for b in crate::backup::list() {
                println!("{}  {:>10}", b.name, b.size);
            }
            0
        }
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    Some(code)
}

async fn status(db: &Db) -> i32 {
    println!("auth mode: {}", db.auth_mode().await.as_str());
    for a in db.accounts().await {
        let kind = if a.idp { "idp" } else { "native" };
        let state = if a.disabled { " (disabled)" } else { "" };
        println!("  {:<24} {:<10} {kind}{state}", a.username, a.role);
    }
    0
}

async fn set_mode(db: &Db, mode: &str) -> i32 {
    let Some(mode) = AuthMode::parse(mode) else {
        eprintln!("unknown mode {mode:?} (native|forward-auth)");
        return 2;
    };
    if mode == AuthMode::ForwardAuth {
        match ForwardAuth::from_env(crate::config::get().trusted_proxies.clone()) {
            Ok(f) if f.configured() => {}
            Ok(_) => eprintln!(
                "warning: no trusted proxy configured (BIFROST_FWD_SECRET_HEADER/BIFROST_FWD_SECRET \
                 or BIFROST_TRUSTED_PROXIES) — nobody will be able to sign in"
            ),
            Err(e) => eprintln!("warning: forward-auth config: {e}"),
        }
    }
    match db.set_auth_mode(mode).await {
        Ok(()) => {
            println!(
                "auth mode set to {} — restart Bifrost to apply",
                mode.as_str()
            );
            0
        }
        Err(e) => {
            eprintln!("failed: {e}");
            1
        }
    }
}

/// An `AuthInner` for minting links outside the server.
fn offline_auth(db: &Db) -> AuthInner {
    AuthInner {
        db: db.clone(),
        mode: AuthMode::Native,
        fwd: ForwardAuth::default(),
        secure_cookies: true,
        throttle: Throttle::default(),
        setup_token: Default::default(),
    }
}

fn print_link(path: &str) {
    match crate::config::var("BIFROST_PUBLIC_URL") {
        Some(base) => println!("{}{path}", base.trim_end_matches('/')),
        None => println!(
            "{path}   (prefix with this instance's URL; set BIFROST_PUBLIC_URL to print it whole)"
        ),
    }
}

async fn reset_link(db: &Db, username: &str) -> i32 {
    let Some(account) = db.account_by_name(username).await else {
        eprintln!("no account {username:?}");
        return 1;
    };
    if !account.has_password && account.idp {
        eprintln!(
            "note: {} is an IdP account; the link sets a password, which is only usable in native mode",
            account.username
        );
    }
    if db.auth_mode().await != AuthMode::Native {
        eprintln!(
            "note: auth mode is not native — `bifrost auth mode native` and restart to use it"
        );
    }
    match crate::account::mint_reset(&offline_auth(db), account.id, None).await {
        Ok(path) => {
            println!(
                "single-use reset link for {}, valid 24 h:",
                account.username
            );
            print_link(&path);
            0
        }
        Err(e) => {
            eprintln!("failed: {e}");
            1
        }
    }
}

async fn create_sysadmin(db: &Db, username: &str) -> i32 {
    if !auth::valid_username(username) {
        eprintln!("usernames are 3–32 letters, digits, '_', '.' or '-'");
        return 2;
    }
    // Random unusable password until the reset link sets a real one.
    let placeholder = match auth::hash_password(&auth::new_token()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed: {e}");
            return 1;
        }
    };
    match db
        .create_account(username, &placeholder, Role::Sysadmin, auth::now())
        .await
    {
        Ok(_) => reset_link(db, username).await,
        Err(e) => {
            eprintln!("failed (username taken?): {e}");
            1
        }
    }
}

async fn set_role(db: &Db, username: &str, role: &str) -> i32 {
    let Some(role) = Role::try_parse(role) else {
        eprintln!("unknown role {role:?} (user|moderator|sysadmin)");
        return 2;
    };
    let Some(account) = db.account_by_name(username).await else {
        eprintln!("no account {username:?} (IdP accounts appear after their first sign-in)");
        return 1;
    };
    match db.set_role(account.id, role).await {
        Ok(()) => {
            println!("{} is now {}", account.username, role.as_str());
            0
        }
        Err(e) => {
            eprintln!("failed: {e}");
            1
        }
    }
}
