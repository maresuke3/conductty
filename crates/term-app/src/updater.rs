//! `conductty update [--check] [--force]`: self-update from GitHub Releases using OS commands
//! (curl / tar / shasum on Unix, curl / PowerShell on Windows) so no extra crates are needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/maresuke3/conductty/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn print_version() {
    println!("conductty {CURRENT_VERSION}");
}

pub fn run(args: &[String]) -> i32 {
    let (mut check, mut force) = (false, false);
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            other => {
                eprintln!(
                    "unknown option for update: {other}\nusage: conductty update [--check] [--force]"
                );
                return 2;
            }
        }
    }
    match update(check, force) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("update failed: {err}");
            1
        }
    }
}

fn update(check: bool, force: bool) -> Result<(), String> {
    let body = run_cmd(Command::new("curl").args([
        "-fsSL",
        "-H",
        "Accept: application/vnd.github+json",
        LATEST_RELEASE_API,
    ]))?;
    let release: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("invalid release JSON: {e}"))?;
    let tag = release["tag_name"]
        .as_str()
        .ok_or("release has no tag_name")?;
    let newer = is_newer(tag, CURRENT_VERSION)
        .ok_or_else(|| format!("cannot compare versions '{tag}' and '{CURRENT_VERSION}'"))?;
    println!("current: {CURRENT_VERSION}, latest: {tag}");

    if check {
        if newer {
            println!("update available: run `conductty update`");
        } else {
            println!("conductty is up to date");
        }
        return Ok(());
    }
    if !newer && !force {
        println!("conductty is up to date (use --force to reinstall)");
        return Ok(());
    }

    let target = target_for(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or("no prebuilt release for this platform")?;
    let assets: Vec<(&str, &str)> = release["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| Some((x["name"].as_str()?, x["browser_download_url"].as_str()?)))
                .collect()
        })
        .unwrap_or_default();
    let (archive_name, archive_url) = select_archive(&assets, target)
        .ok_or_else(|| format!("release {tag} has no archive for {target}"))?;

    let tmp = std::env::temp_dir().join(format!("conductty-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    let result = download_and_install(&tmp, &assets, archive_name, archive_url, tag);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn download_and_install(
    tmp: &Path,
    assets: &[(&str, &str)],
    archive_name: &str,
    archive_url: &str,
    tag: &str,
) -> Result<(), String> {
    // Local file names are fixed so remote asset names never become paths.
    let is_zip = archive_name.ends_with(".zip");
    let archive = tmp.join(if is_zip {
        "archive.zip"
    } else {
        "archive.tar.gz"
    });
    println!("downloading {archive_name} ...");
    download(archive_url, &archive)?;

    match select_sums(assets, std::env::consts::OS) {
        Some(sums_url) => {
            let sums_path = tmp.join("SHA256SUMS.txt");
            download(sums_url, &sums_path)?;
            let sums = fs::read_to_string(&sums_path).map_err(|e| e.to_string())?;
            let expected = find_checksum(&sums, archive_name)
                .ok_or_else(|| format!("{archive_name} not listed in SHA256SUMS"))?;
            let actual = sha256(&archive)?;
            if actual != expected {
                return Err(format!(
                    "checksum mismatch for {archive_name}: expected {expected}, got {actual}"
                ));
            }
            println!("checksum OK");
        }
        None => eprintln!("warning: no SHA256SUMS in release; skipping checksum verification"),
    }

    let extract = tmp.join("extract");
    fs::create_dir_all(&extract).map_err(|e| e.to_string())?;
    extract_archive(&archive, &extract)?;

    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot locate current executable: {e}"))?;
    let install_dir = exe.parent().ok_or("executable has no parent directory")?;

    let suffix = std::env::consts::EXE_SUFFIX;
    let mut files = vec![
        (format!("conductty{suffix}"), true),
        (format!("conductty-server{suffix}"), true),
    ];
    if cfg!(windows) {
        files.push(("WebView2Loader.dll".to_string(), false));
    }
    let mut pairs = Vec::new();
    for (name, required) in &files {
        match find_file(&extract, name) {
            Some(src) => pairs.push((src, install_dir.join(name))),
            None if *required => return Err(format!("{name} not found in archive")),
            None => {}
        }
    }
    replace_files(&pairs)?;

    if let Some(app) = app_bundle_dir(&exe) {
        println!("re-signing {} ...", app.display());
        run_cmd(Command::new("xattr").arg("-cr").arg(app))?;
        run_cmd(
            Command::new("codesign")
                .args(["--force", "--deep", "--sign", "-"])
                .arg(app),
        )?;
    }
    println!("updated to {tag} in {}", install_dir.display());
    Ok(())
}

/// Stage every file next to its destination first, so a write-permission failure
/// leaves the install untouched, then swap them in.
fn replace_files(pairs: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut staged = Vec::new();
    for (src, dest) in pairs {
        let new = with_suffix(dest, ".new");
        if let Err(e) = fs::copy(src, &new) {
            for (n, _) in &staged {
                let _ = fs::remove_file(n);
            }
            return Err(format!(
                "cannot write {}: {e} (try running with sufficient permissions)",
                new.display()
            ));
        }
        staged.push((new, dest.clone()));
    }
    for (new, dest) in &staged {
        swap_in(new, dest)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn swap_in(new: &Path, dest: &Path) -> Result<(), String> {
    // rename() replaces atomically; the running process keeps its old inode.
    fs::rename(new, dest).map_err(|e| format!("replace {}: {e}", dest.display()))
}

#[cfg(windows)]
fn swap_in(new: &Path, dest: &Path) -> Result<(), String> {
    // A running .exe/.dll cannot be overwritten but can be renamed aside.
    let old = with_suffix(dest, ".old");
    let _ = fs::remove_file(&old);
    let moved = dest.exists();
    if moved {
        fs::rename(dest, &old).map_err(|e| format!("rename {}: {e}", dest.display()))?;
    }
    if let Err(e) = fs::rename(new, dest) {
        if moved {
            let _ = fs::rename(&old, dest);
        }
        return Err(format!("replace {}: {e}", dest.display()));
    }
    Ok(())
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn run_cmd(cmd: &mut Command) -> Result<String, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    run_cmd(
        Command::new("curl")
            .args(["-fsSL", "--retry", "2", "-o"])
            .arg(dest)
            .arg(url),
    )
    .map(|_| ())
}

#[cfg(not(windows))]
fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    run_cmd(
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest),
    )
    .map(|_| ())
}

#[cfg(windows)]
fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    powershell(&format!(
        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
        ps_quote(archive),
        ps_quote(dest)
    ))
    .map(|_| ())
}

#[cfg(not(windows))]
fn sha256(path: &Path) -> Result<String, String> {
    let out = run_cmd(Command::new("sha256sum").arg(path))
        .or_else(|_| run_cmd(Command::new("shasum").args(["-a", "256"]).arg(path)))?;
    first_token_lower(&out)
}

#[cfg(windows)]
fn sha256(path: &Path) -> Result<String, String> {
    let out = powershell(&format!(
        "(Get-FileHash -Algorithm SHA256 -LiteralPath {}).Hash",
        ps_quote(path)
    ))?;
    first_token_lower(&out)
}

fn first_token_lower(out: &str) -> Result<String, String> {
    out.split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "empty hash output".to_string())
}

#[cfg(windows)]
fn powershell(script: &str) -> Result<String, String> {
    run_cmd(Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]))
}

#[cfg(windows)]
fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

/// Recursive search; symlinks are ignored because the archive is untrusted.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let ft = entry.file_type().ok()?;
        let path = entry.path();
        if ft.is_file() && entry.file_name() == name {
            return Some(path);
        }
        if ft.is_dir()
            && let Some(found) = find_file(&path, name)
        {
            return Some(found);
        }
    }
    None
}

/// `.../Foo.app/Contents/MacOS/conductty` -> `.../Foo.app`
fn app_bundle_dir(exe: &Path) -> Option<&Path> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_app = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app";
    is_app.then_some(app)
}

/// "v0.2.0-macos" -> [0, 2, 0]. Anything after `-`/`+` is a platform/build label, not a prerelease.
fn parse_version(s: &str) -> Option<Vec<u64>> {
    let core = s.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

fn is_newer(latest: &str, current: &str) -> Option<bool> {
    let (mut l, mut c) = (parse_version(latest)?, parse_version(current)?);
    let n = l.len().max(c.len());
    l.resize(n, 0);
    c.resize(n, 0);
    Some(l > c)
}

fn target_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn select_archive<'a>(assets: &[(&'a str, &'a str)], target: &str) -> Option<(&'a str, &'a str)> {
    let ext = if target.contains("windows") {
        ".zip"
    } else {
        ".tar.gz"
    };
    assets
        .iter()
        .copied()
        .find(|(name, _)| name.contains(target) && name.ends_with(ext))
}

/// Prefer the OS-specific sums file (per-platform releases), fall back to the combined one.
fn select_sums<'a>(assets: &[(&'a str, &'a str)], os: &str) -> Option<&'a str> {
    let os_specific = format!("SHA256SUMS-{os}.txt");
    [os_specific.as_str(), "SHA256SUMS.txt"]
        .iter()
        .find_map(|want| assets.iter().find(|(n, _)| n == want).map(|(_, u)| *u))
}

/// Parses `sha256sum` output lines: `<hash>  <file>` or `<hash> *<file>`.
fn find_checksum(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_handles_prefix_and_platform_suffix() {
        assert_eq!(parse_version("v0.2.0"), Some(vec![0, 2, 0]));
        assert_eq!(parse_version("0.2.0-macos"), Some(vec![0, 2, 0]));
        assert_eq!(parse_version("vx"), None);
        assert_eq!(is_newer("v0.2.0", "0.1.0"), Some(true));
        assert_eq!(is_newer("0.1.0", "v0.1.0"), Some(false));
        assert_eq!(is_newer("v0.1.10", "0.1.9"), Some(true));
        assert_eq!(is_newer("v0.1", "0.1.0"), Some(false));
        assert_eq!(is_newer("v0.1.0-windows", "0.2.0"), Some(false));
        assert_eq!(is_newer("latest", "0.1.0"), None);
    }

    #[test]
    fn target_resolution() {
        assert_eq!(target_for("macos", "aarch64"), Some("aarch64-apple-darwin"));
        assert_eq!(target_for("macos", "x86_64"), Some("x86_64-apple-darwin"));
        assert_eq!(
            target_for("linux", "x86_64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for("windows", "x86_64"),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(target_for("linux", "aarch64"), None);
    }

    #[test]
    fn archive_and_sums_selection() {
        let assets = [
            ("Conductty-v0.2.0-aarch64-apple-darwin.dmg", "u-dmg"),
            ("conductty-v0.2.0-aarch64-apple-darwin.tar.gz", "u-mac"),
            ("conductty-v0.2.0-x86_64-pc-windows-msvc.zip", "u-win"),
            ("SHA256SUMS.txt", "u-sums"),
            ("SHA256SUMS-macos.txt", "u-sums-mac"),
        ];
        assert_eq!(
            select_archive(&assets, "aarch64-apple-darwin"),
            Some(("conductty-v0.2.0-aarch64-apple-darwin.tar.gz", "u-mac"))
        );
        assert_eq!(
            select_archive(&assets, "x86_64-pc-windows-msvc").map(|a| a.1),
            Some("u-win")
        );
        assert_eq!(select_archive(&assets, "x86_64-unknown-linux-gnu"), None);
        assert_eq!(select_sums(&assets, "macos"), Some("u-sums-mac"));
        assert_eq!(select_sums(&assets, "linux"), Some("u-sums"));
        assert_eq!(select_sums(&assets[..3], "linux"), None);
    }

    #[test]
    fn sha256sums_parsing() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  conductty-v1-x.tar.gz\n{b} *conductty-v1-y.zip\nbad  z.zip\n");
        assert_eq!(find_checksum(&sums, "conductty-v1-x.tar.gz"), Some(a));
        assert_eq!(
            find_checksum(&sums, "conductty-v1-y.zip"),
            Some("b".repeat(64))
        );
        assert_eq!(find_checksum(&sums, "z.zip"), None);
        assert_eq!(find_checksum(&sums, "missing.tar.gz"), None);
    }

    #[test]
    fn app_bundle_detection() {
        let exe = Path::new("/Applications/Conductty.app/Contents/MacOS/conductty");
        assert_eq!(
            app_bundle_dir(exe),
            Some(Path::new("/Applications/Conductty.app"))
        );
        assert_eq!(app_bundle_dir(Path::new("/usr/local/bin/conductty")), None);
    }

    #[test]
    fn find_file_and_replace_files_roundtrip() {
        let root =
            std::env::temp_dir().join(format!("conductty-updater-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let src_dir = root.join("extract/conductty-v1-x");
        let dest_dir = root.join("install");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(src_dir.join("conductty"), b"new").unwrap();
        fs::write(dest_dir.join("conductty"), b"old").unwrap();

        let src = find_file(&root.join("extract"), "conductty").unwrap();
        replace_files(&[(src, dest_dir.join("conductty"))]).unwrap();
        assert_eq!(fs::read(dest_dir.join("conductty")).unwrap(), b"new");
        assert!(!dest_dir.join("conductty.new").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
