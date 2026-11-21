use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
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
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub exit_rx: mpsc::Receiver<()>,
}

impl PtySession {
    /// OS に応じたデフォルトシェル（Windows は wsl.exe 等）を PTY 上で起動する。
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
                .unwrap_or("wsl.exe");
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
            CommandBuilder::new(shell)
        };
        cmd.env("TERM", "xterm-256color");

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

        // Windows ConPTY は子プロセス終了後もパイプを閉じないため、child.wait() で終了を検知する
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = child.wait();
            let _ = exit_tx.send(());
        });

        Ok(Self {
            master: pair.master,
            writer,
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
