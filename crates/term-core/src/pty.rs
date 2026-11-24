use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
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
    pub fn spawn(cols: u16, rows: u16, windows_prog: &[String]) -> Result<Self, PtyError> {
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
            unix_shell_command()
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("TERM_PROGRAM", "conductty");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
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

fn unix_shell_command() -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
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
    fn unix_shell_is_login_shell_only_on_macos() {
        let argv = unix_shell_command().get_argv().clone();
        let expected = if cfg!(target_os = "macos") { 2 } else { 1 };
        assert_eq!(argv.len(), expected);
        assert_eq!(
            argv.get(1).is_some_and(|a| a == "-l"),
            cfg!(target_os = "macos")
        );
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
