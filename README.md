# conductty

macOS / Windows (WSL2) 両対応の軽量 GPU ターミナルエミュレータ。  
Ghostty 互換の操作感に加え、**左Drawer（`Master Space` / Workspace 管理）** と **右Drawer（GitHub PR/Issue 連携 + オンデマンド `wry` 内蔵ブラウザ）** を統合しています。

## 起動・テスト

```bash
# 起動（WSL 上なら自動で X11/ソフトGL、macOS/Linux ならネイティブ起動）
make run

# WSL 上から Windows ネイティブ (.exe / Vulkan + WebView2) をクロスビルドして起動
make run-win

# テスト & チェック
make test
make check
```

---

## キーバインド一覧

### 基本・タブ・ペイン操作（通常モード）

| キー | 動作 | 備考 |
| :--- | :--- | :--- |
| `Alt + d` | 右にペイン分割 | 新しいシェルを起動 |
| `Alt + Shift + d` | 下にペイン分割 | 新しいシェルを起動 |
| `Ctrl + h / j / k / l` | 隣接ペインへ移動（`performable:`） | 移動先にペインが無い場合はシェルへ透過 |
| `Ctrl + Shift + H` | **ペイン操作モード (`PANE MODE`)** に入る | 下記の専用キーで連続操作 |
| `Ctrl + Shift + Z` | 現在のペインをズーム / 解除 | `split-preserve-zoom` 対応 |
| `Ctrl + Shift + Space` | **`go_to`** オーバーレイ表示 | 番号キー（`1`〜`9`）でペインへ即ジャンプ / `Esc` で解除 |
| `Ctrl + t` | 新しいタブを作成 | 現在の Workspace に追加 |
| `Ctrl + Shift + E` | **左Drawer操作モード** に入る | 折りたたまれている場合は展開 |
| `Ctrl + Shift + G` | **右Drawer（GitHub）** を開いて操作モードに入る | — |
| `Ctrl + Shift + R` | **`config.conductty` を即時リロード** | テーマ・透過・フォント・キーマップを反映 |

### ペイン操作モード（`Ctrl + Shift + H`）

| キー | 動作 |
| :--- | :--- |
| `h / j / k / l` | 左 / 下 / 上 / 右のペインへ移動 |
| `z` | ズーム切替 |
| `Enter` / `Esc` | ペイン操作モードを終了（通常入力に戻る） |

### 左Drawer 操作モード（`Ctrl + Shift + E`）

| キー | 動作 |
| :--- | :--- |
| `j / k`（または `↓ / ↑`） | Workspace の上下選択 |
| `Enter` / `Space` | 選択した Workspace に切り替え |
| `n` | 新しい Dev Workspace を作成 |
| `r` | 選択中の Workspace 名をリネーム（`Edit` ボタンでも可） |
| `d` | 選択中の Dev Workspace を閉じる（`x` ボタンでも可 / `Master Space` は保護） |
| `c` | 左Drawerを折りたたみ / 展開（`<<` / `>` ボタンでも可） |
| `q` / `Esc` / `Ctrl + Shift + E` | 操作モードを終了（Drawer 自体は開いたまま維持） |

### 右Drawer 操作モード（`Ctrl + Shift + G`）

| キー | 動作 |
| :--- | :--- |
| `j / k`（または `↓ / ↑`） | PR / Issue カードの上下選択（自動スクロール追従） |
| `b` | 選択中のアイテムを **内蔵ブラウザ（`wry`）** でプレビュー |
| `o` / `Enter` | 選択中のアイテムを外部ブラウザで開く |
| `s` | 選択中のアイテム情報（`[repo #num] title (url)`）をアクティブペインへ送信 |
| `r` | GitHub 情報を再取得（Refresh） |
| `q` / `Esc` / `Ctrl + Shift + G` | 右Drawer（および内蔵ブラウザ）を閉じて通常入力に戻る |

### コピー＆ペースト・スクロール

| 操作 | 動作 |
| :--- | :--- |
| マウス左ドラッグ | テキスト選択 ＋ 離した瞬間に自動コピー（`copy-on-select`） |
| `Ctrl + Shift + C` / `Cmd + C` | 選択中のテキストをコピー |
| `Ctrl + V` / `Ctrl + Shift + V` / `Cmd + V` | クリップボードの内容をペースト |
| マウスホイール（ペイン上） | スクロールバック履歴を上下スクロール（最大 2000 行） |
