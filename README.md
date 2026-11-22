# conductty

macOS / Windows (WSL2) / Linux 両対応の軽量 GPU ターミナルエミュレータ。  
Ghostty 互換の操作感に加え、**左Drawer（`Master Space` / Workspace 管理）** と **右Drawer（GitHub PR/Issue 連携 + オンデマンド `wry` 内蔵ブラウザ）** を統合しています。

---

## 主な特徴

- **GPU アクセラレーション描画**: `wgpu` + `egui` + `cosmic-text` による高速なテキスト・UI レンダリングと背景透過に対応
- **Ghostty 互換の操作感**: 二分木ペイン分割、ズーム状態保持（`split_preserve_zoom`）、`performable:` キーバインド、`go_to` 番号ジャンプ、`copy_on_select` をサポート
- **左Drawer（Workspace 管理）**: `Master Space`（誤削除保護）と複数の Dev Workspace の作成・切替・リネーム・クローズをキーボード/マウスで操作可能（`always` / `collapsed` / `hidden`）
- **右Drawer（GitHub 連携 & 内蔵ブラウザ）**: `gh` CLI 経由で Review Requested PR / Assigned PR / Assigned Issue を一覧表示し、アクティブペインへの情報送信や **オンデマンド `wry` 内蔵ブラウザ**（Windows / macOS）でのプレビューに対応
- **Kitty Graphics Protocol**: ターミナル上でのインライン画像表示に対応
- **ホットリロード可能な設定**: `config.conductty`（TOML 形式、`include` 分割対応）を `Ctrl + Shift + R` で即時反映
- **マルチプレクサ構成**: `term-mux` による `conductty-server` バイナリを同梱

---

## 動作要件

- **共通**:
  - GPU（またはソフトウェア OpenGL / Vulkan 環境）
  - 右Drawer（GitHub 連携）を使う場合: [GitHub CLI (`gh`)](https://cli.github.com/) がインストールされ、`gh auth login` 済みであること
- **Windows**:
  - WebView2 ランタイム（内蔵ブラウザ用、Windows 10 / 11 は標準搭載）
  - デフォルトシェル設定で WSL2 を使用する場合は `wsl.exe` が利用可能であること
- **ソースからビルドする場合**:
  - Rust（`stable` / Edition 2024、`mise.toml` あり）
  - Linux の場合は `pkg-config`, `libx11-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev` などの開発用ライブラリ

---

## インストール / ビルドと起動

### リリースバイナリから導入

[GitHub Releases](https://github.com/maresuke3/conductty/releases) からお使いの OS / アーキテクチャ向けのアーカイブ（`conductty` / `conductty-server`）をダウンロードして配置してください。

### ソースからビルド・起動

```bash
# 起動（WSL 上なら自動で X11/ソフトGL、macOS/Linux ならネイティブ起動）
make run

# WSL 環境フラグを明示して起動
make run-wsl

# WSL 上から Windows ネイティブ (.exe / Vulkan + WebView2) をクロスビルドして起動
make run-win

# リリースビルド（target/release/ に conductty と conductty-server を生成）
make build
```

### 開発用コマンド

```bash
# テスト実行
make test

# 型・ビルドチェック
make check

# Clippy による静的解析
make clippy
```

---

## 設定 (`config.conductty`)

設定ファイルは以下の優先順位で読み込まれます（見つからない場合は組み込みデフォルトを使用）。

1. `~/.config/conductty/config.conductty`
2. `~/.config/config.conductty`
3. `~/.conductty/config.conductty`
4. `~/.config.conductty`
5. 組み込みデフォルト設定

リポジトリルートの [`config.conductty`](config.conductty) をベースにコピーして編集できます。`include = "base.toml"` による設定ファイルの分割・継承にも対応しています。

### 主な設定項目

```toml
[features]
multiplexer_mode = false
left_drawer_mode = "collapsed" # "always" | "collapsed" | "hidden"
right_drawer = true
right_drawer_default_open = false
embedded_browser = true
kitty_graphics = true
copy_on_select = true

[appearance]
font_families = ["Cica", "FiraCode Nerd Font", "Fira Code"]
font_size = 18.0
font_thicken = true
adjust_cell_height = 2.0
# プリセット: "TokyoNight Moon" / "TokyoNight" / "Solarized Dark"
theme = "TokyoNight Moon"
background_opacity = 0.85
window_padding_x = 10.0
window_padding_y = 10.0
window_padding_balance = true

# テーマ色を個別に上書きしたい場合
# [theme]
# background = "#222436"
# foreground = "#c8d3f5"
# accent = "#7aa2f7"
# drawer_background = "#1e2030"
# card_background = "#292e42"
# cursor = "#c8d3f5"

[shell]
windows_default_prog = ["wsl.exe", "--cd", "~"]

[window]
save_state = "always"
split_preserve_zoom = "navigation"
macos_option_as_alt = "left"
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
| `Ctrl + Shift + Z` | 現在のペインをズーム / 解除 | `split_preserve_zoom` 対応 |
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
| マウス左ドラッグ | テキスト選択 ＋ 離した瞬間に自動コピー（`copy_on_select`） |
| `Ctrl + Shift + C` / `Cmd + C` | 選択中のテキストをコピー |
| `Ctrl + V` / `Ctrl + Shift + V` / `Cmd + V` | クリップボードの内容をペースト |
| マウスホイール（ペイン上） | スクロールバック履歴を上下スクロール（最大 2000 行） |

---

## アーキテクチャ

一方向依存（Strict DAG）と Functional Core, Imperative Shell 原則に基づいて設計されています。詳細は [`ARCHITECTURE.md`](ARCHITECTURE.md) を参照してください。

- `crates/term-core`: 純粋コア（VTパーサ・グリッド・二分木レイアウト・キー入力判定・設定・PTY抽象化）
- `crates/term-render`: GPU レンダリング・フォント描画（`wgpu` / `cosmic-text`）
- `crates/term-drawer`: GitHub PR/Issue 取得（`gh` CLI 連携）
- `crates/term-mux`: マルチプレクサおよび `conductty-server`
- `crates/term-app`: アプリケーション本体（`winit` / `egui` / `wry` イベントループ統合）

---

## リリース手順

`v*` 形式のタグをプッシュすると GitHub Actions（[`.github/workflows/release.yml`](.github/workflows/release.yml)）が各 OS 向けバイナリをビルドし、GitHub Release を自動作成します。

```bash
git tag v0.1.0
git push origin v0.1.0
```

---

## ライセンス

MIT License
