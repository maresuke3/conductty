use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::thread;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("failed to open pty: {0}")]
    Open(String),
    #[error("failed to spawn shell: {0}")]
    Spawn(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    exited: Arc<AtomicBool>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub exit_rx: mpsc::Receiver<()>,
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if !self.exited.load(Ordering::Acquire)
            && let Some(mut killer) = self.killer.take()
        {
            let _ = killer.kill();
        }
    }
}

impl PtySession {
    /// OS に応じたデフォルトシェル（Windows は powershell.exe 等）を PTY 上で起動する。
    /// cwd を渡すとそのディレクトリでシェルを起動する（無いディレクトリなら渡さなかったのと同じ）
    /// shell_integration が true なら、zsh にシェル連携のスクリプトを読ませる
    pub fn spawn(
        cols: u16,
        rows: u16,
        windows_prog: &[String],
        cwd: Option<&Path>,
        shell_integration: bool,
    ) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))?;

        let mut cmd = if cfg!(target_os = "windows") {
            let prog = windows_prog
                .first()
                .map(String::as_str)
                .unwrap_or("powershell.exe");
            let mut c = CommandBuilder::new(prog);
            for arg in windows_prog.iter().skip(1) {
                c.arg(arg);
            }
            // WSL から .exe を起動した場合の UNC パス (\\wsl.localhost\...) を避ける
            let win_cwd = std::env::var_os("USERPROFILE").unwrap_or_else(|| "C:\\".into());
            c.cwd(win_cwd);
            c
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
            let mut c = unix_shell_command(&shell);
            if shell_integration && let Some(dir) = zsh_integration_dir() {
                for (key, value) in zsh_integration_env(&shell, std::env::var_os("ZDOTDIR"), &dir) {
                    c.env(key, value);
                }
            }
            c
        };
        if let Some(dir) = cwd.filter(|d| d.is_dir()) {
            cmd.cwd(dir);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("TERM_PROGRAM", "conductty");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        if let Some(path) = std::env::current_exe()
            .ok()
            .and_then(|exe| path_with_dir(std::env::var_os("PATH"), exe.parent()?))
        {
            cmd.env("PATH", path);
        }
        if cfg!(target_os = "windows") {
            let mut wslenv = std::env::var("WSLENV").unwrap_or_default();
            for key in ["TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"] {
                if !wslenv
                    .split(':')
                    .any(|item| item.split('/').next() == Some(key))
                {
                    if !wslenv.is_empty() {
                        wslenv.push(':');
                    }
                    wslenv.push_str(key);
                }
            }
            cmd.env("WSLENV", wslenv);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| PtyError::Spawn(e.to_string()))?;
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::Open(e.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::Open(e.to_string()))?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let killer = child.clone_killer();
        let exited = Arc::new(AtomicBool::new(false));
        let exited_clone = Arc::clone(&exited);

        // Windows ConPTY は子プロセス終了後もパイプを閉じないため、child.wait() で終了を検知する
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = child.wait();
            exited_clone.store(true, Ordering::Release);
            let _ = exit_tx.send(());
        });

        Ok(Self {
            master: pair.master,
            writer,
            killer: Some(killer),
            exited,
            output_rx: rx,
            exit_rx,
        })
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::Open(e.to_string()))
    }
}

/// Ghostty 同様、自身のバイナリのディレクトリを PATH 末尾に足してシェルから `conductty` を叩けるようにする。
/// 既に含まれている・PATH 未設定・結合不能な場合は None（PATH を触らない）。
fn path_with_dir(path: Option<OsString>, dir: &Path) -> Option<OsString> {
    let mut paths: Vec<_> = std::env::split_paths(&path?).collect();
    if paths.iter().any(|p| p == dir) {
        return None;
    }
    paths.push(dir.to_path_buf());
    std::env::join_paths(paths).ok()
}

const ZSH_ENV: &str = include_str!("../shell-integration/zsh/.zshenv");
const ZSH_INTEGRATION: &str = include_str!("../shell-integration/zsh/conductty-integration.zsh");

/// zsh のシェル連携のスクリプトを ~/.cache/conductty/shell-integration/zsh に書き出し、そのディレクトリを返す
/// 書き出せないときは None（シェル連携なしで起動する）
fn zsh_integration_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir =
            PathBuf::from(std::env::var_os("HOME")?).join(".cache/conductty/shell-integration/zsh");
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(dir.join(".zshenv"), ZSH_ENV).ok()?;
        std::fs::write(dir.join("conductty-integration.zsh"), ZSH_INTEGRATION).ok()?;
        Some(dir)
    })
    .clone()
}

/// zsh のときだけ、ZDOTDIR を差し替えてシェル連携のスクリプトを読ませる（Ghostty と同じやり方）
/// 元の ZDOTDIR は CONDUCTTY_ZSH_ZDOTDIR で渡し、スクリプトの中で戻す
fn zsh_integration_env(
    shell: &str,
    zdotdir: Option<OsString>,
    dir: &Path,
) -> Vec<(&'static str, OsString)> {
    if Path::new(shell).file_name() != Some("zsh".as_ref()) {
        return Vec::new();
    }
    let mut env = vec![
        ("ZDOTDIR", dir.as_os_str().to_owned()),
        (
            "CONDUCTTY_SHELL_INTEGRATION_DIR",
            dir.as_os_str().to_owned(),
        ),
    ];
    if let Some(zdotdir) = zdotdir {
        env.push(("CONDUCTTY_ZSH_ZDOTDIR", zdotdir));
    }
    env
}

fn unix_shell_command(shell: &str) -> CommandBuilder {
    let mut c = CommandBuilder::new(shell);
    // .app 起動時は launchd の最小 PATH しか持たないため、ログインシェルで ~/.zprofile 等を読ませる
    if cfg!(target_os = "macos") {
        c.arg("-l");
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_integration_env_only_for_zsh_and_keeps_user_zdotdir() {
        let dir = Path::new("/tmp/conductty-zsh");
        assert!(zsh_integration_env("/bin/bash", None, dir).is_empty());
        let env = zsh_integration_env("/opt/homebrew/bin/zsh", Some("/Users/me/.zsh".into()), dir);
        assert_eq!(
            env,
            vec![
                ("ZDOTDIR", OsString::from("/tmp/conductty-zsh")),
                (
                    "CONDUCTTY_SHELL_INTEGRATION_DIR",
                    OsString::from("/tmp/conductty-zsh")
                ),
                ("CONDUCTTY_ZSH_ZDOTDIR", OsString::from("/Users/me/.zsh")),
            ]
        );
        assert_eq!(zsh_integration_env("zsh", None, dir).len(), 2);
    }

    #[test]
    fn unix_shell_is_login_shell_only_on_macos() {
        let argv = unix_shell_command("/bin/zsh").get_argv().clone();
        let expected = if cfg!(target_os = "macos") { 2 } else { 1 };
        assert_eq!(argv.len(), expected);
        assert_eq!(
            argv.get(1).is_some_and(|a| a == "-l"),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn path_with_dir_appends_once_at_end() {
        let dir = std::env::temp_dir().join("conductty-bin");
        let other = std::env::temp_dir().join("other-bin");
        let path = std::env::join_paths([&other]).unwrap();

        let appended = path_with_dir(Some(path), &dir).unwrap();
        let entries: Vec<_> = std::env::split_paths(&appended).collect();
        assert_eq!(entries, vec![other.clone(), dir.clone()]);

        assert_eq!(path_with_dir(Some(appended), &dir), None);
        assert_eq!(path_with_dir(None, &dir), None);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_pty_session_kills_child_and_notifies_exit() {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("60");
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);

        let killer = child.clone_killer();
        let exited = Arc::new(AtomicBool::new(false));
        let exited_clone = Arc::clone(&exited);
        let writer = pair.master.take_writer().unwrap();
        let (_tx, rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = child.wait();
            exited_clone.store(true, Ordering::Release);
            let _ = exit_tx.send(());
        });

        let mut session = PtySession {
            master: pair.master,
            writer,
            killer: Some(killer),
            exited,
            output_rx: rx,
            exit_rx,
        };
        let (_dummy_tx, dummy_rx) = mpsc::channel();
        let exit_rx = std::mem::replace(&mut session.exit_rx, dummy_rx);
        drop(session);

        assert!(
            exit_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok(),
            "child process should terminate promptly when PtySession is dropped"
        );
    }
}
