//! Secret storage in the OS keyring, through its CLI so no native crate is needed:
//! - macOS: the Keychain via `/usr/bin/security` (same approach as paperplane's
//!   `src/secrets/keychain.ts`). The Keychain ACL belongs to `security`, so
//!   rebuilding insta never brings back the "allow access" prompt.
//! - Linux: the Secret Service (GNOME Keyring, KWallet) via libsecret's `secret-tool`.

use anyhow::{Result, bail};
use async_trait::async_trait;

pub const SERVICE: &str = "insta";
pub const ACCOUNT_SESSIONID: &str = "sessionid";
pub const ACCOUNT_CSRFTOKEN: &str = "csrftoken";
pub const ACCOUNT_DS_USER_ID: &str = "ds_user_id";
pub const ACCOUNT_BEARER: &str = "bearer_token";
pub const ACCOUNT_OPENAI_KEY: &str = "openai_key";

#[derive(Debug)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput>;
}

pub const MISSING_SECURITY: &str = "/usr/bin/security not found: insta needs the macOS Keychain";
pub const MISSING_SECRET_TOOL: &str = "secret-tool not found: install it (Ubuntu/Debian: sudo apt install libsecret-tools) and make sure a keyring is running";

/// Runs one keyring CLI. `missing` is the error shown when it isn't installed.
pub struct Cli {
    program: &'static str,
    missing: &'static str,
}

impl Cli {
    pub fn new(program: &'static str, missing: &'static str) -> Cli {
        Cli { program, missing }
    }
}

#[async_trait]
impl CommandRunner for Cli {
    async fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput> {
        use tokio::io::AsyncWriteExt;
        let spawned = tokio::process::Command::new(self.program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("{}", self.missing),
            Err(e) => return Err(e.into()),
        };
        if let Some(mut pipe) = child.stdin.take() {
            // security may exit before reading stdin; a broken pipe is reported
            // through the exit code instead.
            let _ = pipe.write_all(stdin.unwrap_or("").as_bytes()).await;
        }
        let out = child.wait_with_output().await?;
        Ok(CmdOutput {
            code: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn get(&self, account: &str) -> Result<Option<String>>;
    async fn set(&self, account: &str, value: &str) -> Result<()>;
    async fn delete(&self, account: &str) -> Result<()>;
}

pub struct Keychain<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> Keychain<R> {
    pub fn new(runner: R) -> Self {
        Keychain { runner }
    }
}

#[async_trait]
impl<R: CommandRunner> SecretStore for Keychain<R> {
    async fn get(&self, account: &str) -> Result<Option<String>> {
        let r = self.runner.run(&["find-generic-password", "-s", SERVICE, "-a", account, "-w"], None).await?;
        match r.code {
            0 => Ok(Some(r.stdout.strip_suffix('\n').unwrap_or(&r.stdout).to_string())),
            ITEM_NOT_FOUND => Ok(None),
            code => bail!("keychain read failed for {account} (exit {code})"),
        }
    }

    async fn set(&self, account: &str, value: &str) -> Result<()> {
        assert_safe(account)?;
        assert_safe(value)?;
        let line = format!("add-generic-password -U -s {SERVICE} -a {account} -w {value}\n");
        let r = self.runner.run(&["-i"], Some(&line)).await?;
        if r.code != 0 {
            bail!("keychain write failed for {account} (exit {})", r.code);
        }
        Ok(())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let r = self.runner.run(&["delete-generic-password", "-s", SERVICE, "-a", account], None).await?;
        if r.code != 0 && r.code != ITEM_NOT_FOUND {
            bail!("keychain delete failed for {account} (exit {})", r.code);
        }
        Ok(())
    }
}

/// Linux Secret Service through `secret-tool`. Values go on stdin, never argv,
/// so they don't show up in `ps`; no character restrictions are needed.
pub struct SecretTool<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> SecretTool<R> {
    pub fn new(runner: R) -> Self {
        SecretTool { runner }
    }
}

#[async_trait]
impl<R: CommandRunner> SecretStore for SecretTool<R> {
    async fn get(&self, account: &str) -> Result<Option<String>> {
        let r = self.runner.run(&["lookup", "service", SERVICE, "account", account], None).await?;
        match r.code {
            0 => {
                let v = r.stdout.strip_suffix('\n').unwrap_or(&r.stdout);
                Ok((!v.is_empty()).then(|| v.to_string()))
            }
            // secret-tool exits 1 with no output when nothing matches.
            1 if r.stdout.is_empty() => Ok(None),
            code => bail!("keyring read failed for {account} (secret-tool exit {code})"),
        }
    }

    async fn set(&self, account: &str, value: &str) -> Result<()> {
        if value.is_empty() {
            bail!("refusing to store an empty value for {account}");
        }
        let label = format!("{SERVICE} {account}");
        let r =
            self.runner.run(&["store", "--label", &label, "service", SERVICE, "account", account], Some(value)).await?;
        if r.code != 0 {
            bail!("keyring write failed for {account} (secret-tool exit {}): is a keyring unlocked?", r.code);
        }
        Ok(())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let r = self.runner.run(&["clear", "service", SERVICE, "account", account], None).await?;
        if r.code > 1 {
            bail!("keyring delete failed for {account} (secret-tool exit {})", r.code);
        }
        Ok(())
    }
}

/// The OS keyring for this platform.
pub fn default_store() -> Result<Box<dyn SecretStore>> {
    if cfg!(target_os = "macos") {
        Ok(Box::new(Keychain::new(Cli::new("/usr/bin/security", MISSING_SECURITY))))
    } else if cfg!(target_os = "linux") {
        Ok(Box::new(SecretTool::new(Cli::new("secret-tool", MISSING_SECRET_TOOL))))
    } else {
        bail!("insta supports macOS (Keychain) and Linux (secret-tool) only")
    }
}

/// security(1) exit status for errSecItemNotFound.
const ITEM_NOT_FOUND: i32 = 44;

/// Values are interpolated into a `security -i` command line, so only allow
/// characters that can't start a new argument or command. `%` is allowed
/// because the `sessionid` cookie is URL-encoded (`%3A` separators).
pub fn assert_safe(value: &str) -> Result<()> {
    let ok = !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_.:%-".contains(c));
    if !ok {
        bail!("keychain values must be token characters (letters, digits, + / = _ . : % -)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        code: i32,
        stdout: String,
        calls: Mutex<Vec<(Vec<String>, Option<String>)>>,
    }

    #[async_trait]
    impl CommandRunner for &FakeRunner {
        async fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<CmdOutput> {
            self.calls.lock().unwrap().push((args.iter().map(|s| s.to_string()).collect(), stdin.map(str::to_string)));
            Ok(CmdOutput { code: self.code, stdout: self.stdout.clone() })
        }
    }

    #[tokio::test]
    async fn get_returns_none_on_exit_44() {
        let r = FakeRunner { code: 44, ..Default::default() };
        assert_eq!(Keychain::new(&r).get(ACCOUNT_SESSIONID).await.unwrap(), None);
    }

    #[tokio::test]
    async fn get_strips_trailing_newline() {
        let r = FakeRunner { stdout: "sess-1\n".into(), ..Default::default() };
        assert_eq!(Keychain::new(&r).get(ACCOUNT_SESSIONID).await.unwrap().as_deref(), Some("sess-1"));
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls[0].0, ["find-generic-password", "-s", "insta", "-a", "sessionid", "-w"]);
    }

    #[tokio::test]
    async fn set_writes_via_stdin_not_argv() {
        let r = FakeRunner::default();
        Keychain::new(&r).set(ACCOUNT_SESSIONID, "sess-abc").await.unwrap();
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls[0].0, ["-i"]);
        assert_eq!(calls[0].1.as_deref(), Some("add-generic-password -U -s insta -a sessionid -w sess-abc\n"));
    }

    #[tokio::test]
    async fn set_rejects_unsafe_chars() {
        let r = FakeRunner::default();
        let k = Keychain::new(&r);
        assert!(k.set(ACCOUNT_SESSIONID, "a b").await.is_err());
        assert!(k.set(ACCOUNT_SESSIONID, "a;b").await.is_err());
        assert!(k.set(ACCOUNT_SESSIONID, "").await.is_err());
        assert!(r.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn set_accepts_percent_encoded_cookie() {
        let r = FakeRunner::default();
        Keychain::new(&r).set(ACCOUNT_SESSIONID, "001%3Aabc%3A12%3Adef").await.unwrap();
    }

    #[tokio::test]
    async fn delete_ignores_not_found() {
        let r = FakeRunner { code: 44, ..Default::default() };
        Keychain::new(&r).delete(ACCOUNT_SESSIONID).await.unwrap();
    }

    #[tokio::test]
    async fn secret_tool_get_looks_up_service_and_account() {
        let r = FakeRunner { stdout: "sess-1".into(), ..Default::default() };
        assert_eq!(SecretTool::new(&r).get(ACCOUNT_SESSIONID).await.unwrap().as_deref(), Some("sess-1"));
        assert_eq!(r.calls.lock().unwrap()[0].0, ["lookup", "service", "insta", "account", "sessionid"]);
    }

    #[tokio::test]
    async fn secret_tool_get_returns_none_when_missing() {
        let r = FakeRunner { code: 1, ..Default::default() };
        assert_eq!(SecretTool::new(&r).get(ACCOUNT_SESSIONID).await.unwrap(), None);
    }

    #[tokio::test]
    async fn secret_tool_set_passes_value_on_stdin_only() {
        let r = FakeRunner::default();
        SecretTool::new(&r).set(ACCOUNT_CSRFTOKEN, "csrf-abc%2F").await.unwrap();
        let calls = r.calls.lock().unwrap();
        assert_eq!(calls[0].0, ["store", "--label", "insta csrftoken", "service", "insta", "account", "csrftoken"]);
        assert_eq!(calls[0].1.as_deref(), Some("csrf-abc%2F"));
        assert!(!calls[0].0.iter().any(|a| a.contains("abc")));
    }

    #[tokio::test]
    async fn secret_tool_set_fails_on_nonzero_exit() {
        let r = FakeRunner { code: 1, ..Default::default() };
        assert!(SecretTool::new(&r).set(ACCOUNT_SESSIONID, "sess-1").await.is_err());
    }

    #[tokio::test]
    async fn secret_tool_delete_clears_the_item() {
        let r = FakeRunner::default();
        SecretTool::new(&r).delete(ACCOUNT_SESSIONID).await.unwrap();
        assert_eq!(r.calls.lock().unwrap()[0].0, ["clear", "service", "insta", "account", "sessionid"]);
    }

    #[tokio::test]
    async fn missing_program_explains_how_to_install() {
        let err = Cli::new("/nonexistent/secret-tool", MISSING_SECRET_TOOL).run(&["lookup"], None).await.unwrap_err();
        assert!(err.to_string().contains("libsecret-tools"));
    }
}

/// In-memory `SecretStore` shared by tests in other modules.
#[cfg(test)]
pub mod tests_support {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MemSecrets(pub Mutex<HashMap<String, String>>);

    #[async_trait]
    impl SecretStore for MemSecrets {
        async fn get(&self, a: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(a).cloned())
        }
        async fn set(&self, a: &str, v: &str) -> Result<()> {
            self.0.lock().unwrap().insert(a.into(), v.into());
            Ok(())
        }
        async fn delete(&self, a: &str) -> Result<()> {
            self.0.lock().unwrap().remove(a);
            Ok(())
        }
    }
}
