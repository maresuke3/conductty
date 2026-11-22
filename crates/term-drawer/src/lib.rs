use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitHubItemKind {
    ReviewRequestedPr,
    AssignedPr,
    AssignedIssue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubItem {
    pub kind: GitHubItemKind,
    pub number: u64,
    pub title: String,
    pub repo: String,
    pub url: String,
}

#[derive(Debug, Clone, Default)]
pub struct DrawerSnapshot {
    pub items: Vec<GitHubItem>,
    pub is_loading: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRepo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct GhSearchRow {
    number: u64,
    title: String,
    url: String,
    repository: GhRepo,
}

pub fn parse_gh_search_json(
    json_str: &str,
    kind: GitHubItemKind,
) -> Result<Vec<GitHubItem>, serde_json::Error> {
    let rows: Vec<GhSearchRow> = serde_json::from_str(json_str)?;
    Ok(rows
        .into_iter()
        .map(|r| GitHubItem {
            kind,
            number: r.number,
            title: r.title,
            repo: r.repository.name_with_owner,
            url: r.url,
        })
        .collect())
}

fn apply_no_window(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(target_os = "windows"))]
    let _ = cmd;
}

async fn run_gh_search(args: &[&str], kind: GitHubItemKind) -> Result<Vec<GitHubItem>, String> {
    let mut gh_cmd = Command::new("gh");
    gh_cmd.args(args);
    apply_no_window(&mut gh_cmd);
    let output = match gh_cmd.output().await {
        Ok(out) => out,
        Err(e) if cfg!(target_os = "windows") && e.kind() == std::io::ErrorKind::NotFound => {
            // Windows 側に gh.exe が無ければ、引数クォート崩れを防ぐため `wsl.exe sh -s` の stdin 経由で実行する
            use tokio::io::AsyncWriteExt;
            let escaped_args: Vec<String> = args
                .iter()
                .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
                .collect();
            let script = format!(
                "PATH=\"$HOME/.local/share/mise/shims:/home/linuxbrew/.linuxbrew/bin:$PATH\" exec gh {}\n",
                escaped_args.join(" ")
            );
            let mut wsl_cmd = Command::new("wsl.exe");
            wsl_cmd
                .args(["sh", "-s"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            apply_no_window(&mut wsl_cmd);
            let mut child = wsl_cmd
                .spawn()
                .map_err(|we| format!("wsl gh 起動失敗: {we}"))?;

            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(script.as_bytes()).await;
            }
            child
                .wait_with_output()
                .await
                .map_err(|we| format!("wsl gh 待機失敗: {we}"))?
        }
        Err(e) => return Err(format!("gh 起動失敗: {e}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("gh 終了コード: {}", output.status)
        } else {
            stderr
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_gh_search_json(&stdout, kind).map_err(|e| format!("JSON パース失敗: {e}"))
}

/// 担当PR・レビュー依頼PR・担当Issueを並列取得してスナップショットを返す。
pub async fn fetch_github_snapshot() -> DrawerSnapshot {
    let review_fut = run_gh_search(
        &[
            "search",
            "prs",
            "--review-requested",
            "@me",
            "--state",
            "open",
            "--limit",
            "15",
            "--json",
            "number,title,url,repository",
        ],
        GitHubItemKind::ReviewRequestedPr,
    );
    let assigned_pr_fut = run_gh_search(
        &[
            "search",
            "prs",
            "--assignee",
            "@me",
            "--state",
            "open",
            "--limit",
            "15",
            "--json",
            "number,title,url,repository",
        ],
        GitHubItemKind::AssignedPr,
    );
    let issue_fut = run_gh_search(
        &[
            "search",
            "issues",
            "--assignee",
            "@me",
            "--state",
            "open",
            "--limit",
            "15",
            "--json",
            "number,title,url,repository",
        ],
        GitHubItemKind::AssignedIssue,
    );

    let (rev_res, pr_res, issue_res) = tokio::join!(review_fut, assigned_pr_fut, issue_fut);
    let mut items = Vec::new();
    let mut errors = Vec::new();

    for res in [rev_res, pr_res, issue_res] {
        match res {
            Ok(mut list) => items.append(&mut list),
            Err(e) => {
                if !errors.contains(&e) {
                    errors.push(e);
                }
            }
        }
    }

    DrawerSnapshot {
        items,
        is_loading: false,
        last_error: if errors.is_empty() {
            None
        } else {
            Some(errors.join(" / "))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gh_search_output() {
        let raw = r#"[
            {
                "number": 42,
                "title": "Add right drawer MVP",
                "url": "https://github.com/maresuke3/conductty/pull/42",
                "repository": { "nameWithOwner": "maresuke3/conductty" }
            }
        ]"#;
        let items = parse_gh_search_json(raw, GitHubItemKind::AssignedPr).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 42);
        assert_eq!(items[0].repo, "maresuke3/conductty");
        assert_eq!(items[0].kind, GitHubItemKind::AssignedPr);
    }
}
