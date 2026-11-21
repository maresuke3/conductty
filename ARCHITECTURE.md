# `conductty` アーキテクチャ設計原則

コードベースが肥大化しても安全に改修・拡張できるよう、以下の2つの原則を厳守する。

## 1. 厳格な一方向依存（Strict DAG）

```text
          term-app  (Composition Root: winit / wgpu / egui / wry / tokio)
         /    |    \
        v     v     v
term-render  term-drawer   (互いに依存しない)
        \     |     /
         v    v    v
          term-core        (純粋コア + PTY抽象化のみ。UI/GPU/HTTPを知らない)
```

- **`term-core`**: `wgpu` / `winit` / `egui` / `tokio` / `wry` への依存を**一切禁止**する。
- **`term-render`**: `term-core` のデータ（`TerminalGrid` スナップショット、`LayoutNode` 矩形、`AppearanceConfig`）を**読むだけ**。`term-drawer` や `portable-pty` を知らない。
- **`term-drawer`**: `term-core` の型（必要ならIDや設定のみ）に依存する。`term-render` を知らない。
- **`term-app`**: 唯一の結合点（Composition Root）。各クレートを繋ぐイベントループとスレッド間チャネルのみを持つ。

## 2. Functional Core, Imperative Shell（純粋コア＋副作用の外殻）

- **純粋関数 / 決定論的ステートマシン**（テスト可能・副作用ゼロ）:
  - `term-core/src/layout.rs`: 二分木分割・矩形計算・隣接ペイン判定
  - `term-core/src/input.rs`: キーイベント + モード + レイアウト → `InputDecision`
  - `term-core/src/grid.rs`: VTバイト列 → `TerminalGrid` セル状態更新
- **副作用（Imperative Shell）**:
  - PTY I/O (`term-core/src/pty.rs`)、`gh` CLI実行 (`term-drawer`)、GPU描画・OSウィンドウ (`term-app`) はすべて外殻に押し出し、メッセージ/値として純粋コアとやり取りする。
