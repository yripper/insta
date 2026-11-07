//! Command line: `start` (daemon), `login`, `logout`, `status`, `token`,
//! `set-openai-key`, `probe`.

pub mod daemon;
pub mod probe;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::config::{Config, Paths};
use crate::ig::HttpInstagram;
use crate::ig::types::{Credentials, Me};
use crate::mcp::state::{Connector, HttpConnector, load_credentials};
use crate::secrets::{
    ACCOUNT_BEARER, ACCOUNT_CSRFTOKEN, ACCOUNT_DS_USER_ID, ACCOUNT_OPENAI_KEY, ACCOUNT_SESSIONID, SecretStore,
    assert_safe, default_store,
};
use crate::store::Store;

#[derive(Parser)]
#[command(
    name = "insta",
    version,
    about = "Local MCP server that lets Claude triage and answer your Instagram DMs, as you"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the MCP server (what pm2 runs).
    Start,
    /// Save your Instagram browser session (sessionid, csrftoken, ds_user_id cookies) to the OS keyring.
    Login,
    /// Remove the Instagram session from the OS keyring.
    Logout,
    /// Show whether the session works.
    Status,
    /// Print the MCP bearer token (for `claude mcp add`).
    Token,
    /// Save an OpenAI API key to the OS keyring, for transcribing voice notes.
    SetOpenaiKey,
    /// Print the structure (no values) of Instagram's responses.
    Probe,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginOutcome {
    Validated(Me),
    /// Saved, but the Instagram client can't check sessions yet.
    ValidationPending,
}

/// Cookie values go into a `security -i` line on macOS, so they must be plain
/// token characters; this also catches pasted quotes or whitespace.
pub fn check_credentials(c: &Credentials) -> Result<()> {
    for (name, v) in [("sessionid", &c.sessionid), ("csrftoken", &c.csrftoken), ("ds_user_id", &c.ds_user_id)] {
        if assert_safe(v).is_err() {
            bail!("{name} looks wrong: paste only the cookie's value (letters, digits, % : _ - .)");
        }
    }
    Ok(())
}

/// Validates the session (once the client can) and saves it. A login also
/// lifts a challenge stop: the user has checked Instagram in the browser.
pub async fn save_login(
    connector: &dyn Connector,
    secrets: &dyn SecretStore,
    store: &Store,
    creds: Credentials,
) -> Result<LoginOutcome> {
    check_credentials(&creds)?;
    let outcome = match connector.connect(creds.clone()).await {
        Ok((_, me)) => LoginOutcome::Validated(me),
        Err(e) if e.is_not_implemented() => LoginOutcome::ValidationPending,
        Err(e) => return Err(e.into()),
    };
    secrets.set(ACCOUNT_SESSIONID, &creds.sessionid).await?;
    secrets.set(ACCOUNT_CSRFTOKEN, &creds.csrftoken).await?;
    secrets.set(ACCOUNT_DS_USER_ID, &creds.ds_user_id).await?;
    store.clear_challenge()?;
    Ok(outcome)
}

fn prompt(label: &str) -> Result<String> {
    Ok(rpassword::prompt_password(format!("{label} (hidden): "))?.trim().to_string())
}

async fn login() -> Result<()> {
    println!("Open https://www.instagram.com in your browser, logged in, then the dev tools");
    println!("(Cmd+Option+I on macOS, Ctrl+Shift+I on Linux): Application > Cookies > https://www.instagram.com.");
    println!("Copy the Value of each cookie below.");
    let creds = Credentials {
        sessionid: prompt("sessionid")?,
        csrftoken: prompt("csrftoken")?,
        ds_user_id: prompt("ds_user_id")?,
    };
    let k = default_store()?;
    let store = Store::open(&Paths::resolve()?.db)?;
    match save_login(&HttpConnector, k.as_ref(), &store, creds).await? {
        LoginOutcome::Validated(me) => {
            println!("Logged in as @{}. A running daemon picks this up on the next call.", me.username)
        }
        LoginOutcome::ValidationPending => {
            println!("Saved to the keyring. Validation is pending: insta can't check sessions with Instagram yet.")
        }
    }
    Ok(())
}

async fn set_openai_key() -> Result<()> {
    println!("Paste an OpenAI API key (https://platform.openai.com/api-keys). It's used only by transcribe_voice.");
    let key = prompt("OpenAI API key")?;
    if assert_safe(&key).is_err() {
        bail!("that doesn't look like an API key (letters, digits, - and _ only)");
    }
    default_store()?.set(ACCOUNT_OPENAI_KEY, &key).await?;
    println!("Saved to the keyring. No restart needed.");
    Ok(())
}

async fn status() -> Result<()> {
    let k = default_store()?;
    let paths = Paths::resolve()?;
    println!("port: {}", Config::load(&paths)?.port);
    let set = |b: bool, missing: &'static str| if b { "set" } else { missing };
    println!("bearer token: {}", set(k.get(ACCOUNT_BEARER).await?.is_some(), "missing (created on first start)"));
    println!("openai key: {}", set(k.get(ACCOUNT_OPENAI_KEY).await?.is_some(), "missing (run `insta set-openai-key`)"));
    if Store::open(&paths.db)?.challenge_at()?.is_some() {
        println!("instagram: stopped after a challenge; check Instagram in the browser, then run `insta login`");
        return Ok(());
    }
    match load_credentials(k.as_ref()).await? {
        None => println!("instagram: not logged in: run `insta login`"),
        Some(c) => match HttpConnector.connect(c).await {
            Ok((_, me)) => println!("instagram: logged in as @{}", me.username),
            Err(e) => println!("instagram: {e}"),
        },
    }
    Ok(())
}

async fn logout() -> Result<()> {
    let k = default_store()?;
    for a in [ACCOUNT_SESSIONID, ACCOUNT_CSRFTOKEN, ACCOUNT_DS_USER_ID] {
        k.delete(a).await?;
    }
    println!("Instagram session removed from the keyring.");
    Ok(())
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Start => daemon::start().await,
        Command::Login => login().await,
        Command::Logout => logout().await,
        Command::Status => status().await,
        Command::Token => {
            println!("{}", daemon::ensure_bearer_token(default_store()?.as_ref()).await?);
            Ok(())
        }
        Command::SetOpenaiKey => set_openai_key().await,
        Command::Probe => {
            let Some(creds) = load_credentials(default_store()?.as_ref()).await? else {
                bail!("not logged in: run `insta login`");
            };
            probe::probe(&HttpInstagram::new(creds)?).await
        }
    }
}

#[cfg(test)]
mod tests;
