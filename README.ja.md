<!-- markdownlint-disable -->
<div align="center">

[🇨🇳 中文](README.md) · [🇬🇧 English](README.en.md) · [🇯🇵 日本語](README.ja.md)

---

```
╔══════════════════════════════════════════════════════════════════════╗
║                                                                    ║
║        ██╗      █████╗ ███████╗██╗    ██╗                          ║
║        ██║     ██╔══██╗██╔════╝██║    ██║                          ║
║        ██║     ███████║█████╗  ██║ █╗ ██║                          ║
║        ██║     ██╔══██║██╔══╝  ██║███╗██║                          ║
║        ███████╗██║  ██║███████╗╚███╔███╔╝                          ║
║        ╚══════╝╚═╝  ╚═╝╚══════╝ ╚══╝╚══╝                           ║
║                                                                    ║
║        LLM · Agent · CLI · Rust · Multi-Agent · 6 Roles           ║
║        デュアルプロトコル · 6 ツール · 6 ロール編成 · TUI          ║
║                                                                    ║
╚══════════════════════════════════════════════════════════════════════╝
```

## 🦀 Rust マルチエージェント CLI · デュアルプロトコル · 6 ロール編成

</div>
<!-- markdownlint-restore -->

<div align="center">

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork)
[![AI Agent Coded](https://img.shields.io/badge/AI--Agent-100%25-ff6b6b)](CLAUDE.md)
[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Chinese](https://img.shields.io/badge/lang-中文-red)](README.md) [![English](https://img.shields.io/badge/lang-English-blue)](README.en.md) [![日本語](https://img.shields.io/badge/lang-日本語-green)](README.ja.md)

</div>

---

> **🤖 100% AI Agent 自動プログラミング** —— 人間の手によるコードは一行もなし。古法プログラミングなし。
> プロジェクト全体（Rust ソース、マルチエージェントアーキテクチャ、TUI レンダリングエンジン、
> ツールシステム、自動テスト、CI スクリプト、ドキュメント）はすべて AI Agent（Claude Code など）が
> 自律的に記述・コンパイル・テスト・リファクタリング・デプロイしたものです。
>
> **⏱️ 14 ラウンドのディープリサーチ · 82+ ディメンション · 635 gap のナレッジベース** —— このリポジトリは
> 「一回のセッションの産物」ではなく、14 ラウンドにわたる 82+ ディメンションの継続的なディープリサーチ、
> 635 の laew gap を蓄積した「Agent プログラミング」能力の完全なデモンストレーションです。
> 各ラウンドで Agent は前回の産出を読み取り、新しいディメンションを計画、専門レポートを作成、
> ナレッジベースに書き込み、git commit します。

---

## 🤖 100% Agent 自動プログラミング —— このリポジトリの核心ハイライト

> **これは一人の人間が書いたコードではありません。AI Agent チームが 24 時間休まずプログラミングした作品です。**

このリポジトリのすべてのコードは AI Agent によって自動記述されています：

- **手作業コーディングなし**：人間のプログラマが Rust / TOML / Shell / Markdown を一行も書いていません。
- **Agent 協力**：複数のサブエージェント責任ライン（アーキテクチャ、TUI、ツール、プロトコル、テスト、ドキュメント）に分割し、それぞれが独立して作業し、自動コミット。
- **自己テスト・自己修正**：Agent が `cargo test` と `bash testReport/run_e2e.sh` を自動実行し、バグの発見→特定→修正→回帰検証を自律的に行います。
- **自己デプロイ**：`rebuild_restart_app.sh` は Agent が記述。`cargo build --release` → `./laew` コピー → 再起動を一コマンドで。
- **継続的イテレーション**：`CLAUDE.md` に膨大な Agent 教訓が記録されており、それぞれは Agent がつまずいた後に自動記述。将来の Agent が自動読み込みして再犯を防止。
- **ナレッジベース**：`docs/` 下に 80+ のリサーチドキュメント / 約 160k 行、すべて Agent 産出。

> **リポジトリデータ**：Rust マルチエージェント CLI、6 ツール、6 ロール編成、デュアルプロトコル、TUI サブスクリーン自動化。
> **これらすべてに、人間が手書した文字は一文字もありません。**

---

## 🎯 プロジェクトポジショニングとコア機能

`laew`（**L**lm **A**gent **E**mergent **W**ork）は Rust ベースの LLM マルチエージェント CLI で、
**Anthropic** と **OpenAI** のデュアルプロトコルをサポート、6 つの組み込みツールコールを備え、
TUI マルチターン会話、`-p` 単一ターンタスク、`-f` ファイルプロンプトの 3 モードを提供します。

| 機能 | 説明 |
|------|------|
| 🧠 **デュアルプロトコル** | Anthropic（anthropic-messages）+ OpenAI（openai-completions）、統一メッセージモデルでプロトコル差異を隔離 |
| 🛠️ **6 ツール** | Bash / Read / Write / Edit / Glob / Grep |
| 🤖 **6 ロールマルチエージェント** | Yolo / Plan / Main-Work / SubAgent-Work / Quality-Check / SessionContext |
| 📊 **3 ティア分類** | simple / medium / hard、Yolo が自動分類して階層編成 |
| 🖥️ **TUI** | crossterm 独立レンダリングエンジン、alternate screen + raw mode + Screen スタック + Tab フォーム |
| 💾 **SQLite 永続化** | ルートディレクトリの `LsmAgentEmergentWork.db`、設定ファイルなし |
| 🔍 **プロジェクトコンテキスト注入** | 5 レベルチェーン CLAUDE.md → AGENTS.md → README.md → 自動生成 → 空、セッションごとに冪等注入 |

---

## 🤖 マルチエージェントアーキテクチャ —— 6 ロールの同ステージ編成

`MultiAgentOrchestrator` が総編成：ユーザー入力 → プロジェクトコンテキスト注入 →
Yolo 3 ティア分類 → simple（SubAgent）/ medium（Main → SubAgent）/
hard（Plan → Main → SubAgent）→ Quality-Check → SessionContext 締めくくり。

| ロール | 責務 | ツール |
|------|------|------|
| 🎯 **Yolo Agent** | エントリレイヤ：目的→目標→意図の 3 ステップ分析 / 3 ティア分類 / 失敗リフラックス | Read |
| 🗺️ **Plan Agent** | 計画レイヤ：hard タスクで Markdown プランを `plans/` に出力 | Read / Write |
| 🔄 **Main-Work Agent** | フローレイヤ：WorkFlow リストに分割 | Bash / Read |
| ⚡ **SubAgent-Work Agent** | 実行レイヤ最小単位：各フローに 1 つの SubAgent を派遣 | Bash / Read / Write |
| ✅ **Quality-Check Agent** | QC レイヤ：各実行ユニット完了後必須 QC | オプション Read |
| 🧠 **SessionContext Agent** | セッションレイヤ：タスク完了後に `session_memory` に要約書き込み | ツールなし |

### 編成トポロジー

```text
ユーザー入力
   │
   ▼
プロジェクトコンテキスト注入（5 レベルチェーン、冪等）
   │
   ▼
Yolo ──→ 分類: simple / medium / hard
   │
   ├─ simple ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   ├─ medium  ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
   │
   └─ hard    ──→ Plan ──→ Main-Work ──→ SubAgent-Work ──→ Quality-Check ──→ SessionContext
```

---

## ⚙️ テックスタック

| モジュール | 選定 |
|------|------|
| 🦀 言語 | Rust 1.75+ |
| ⌨️ TUI | crossterm（独立 CLI レンダリングエンジン：Screen trait + Frame + alternate screen + raw mode） |
| 📟 CLI | clap（derive スタイル：TUI / `-p` / `-f` / `provider` サブコマンド） |
| 💾 データベース | rusqlite（SQLite、`LsmAgentEmergentWork.db`） |
| 🌐 HTTP | reqwest（Anthropic / OpenAI デュアルプロトコルクライアント） |
| 📡 プロトコル | Anthropic Messages + OpenAI Chat Completions（統一メッセージモデル `llm/mod.rs`） |
| 🧪 テスト | `cargo test` ユニット + `testReport/run_e2e.sh` E2E（mock LLM + tmux サブスクリーン自動化） |

---

## 🚀 クイックスタート

### 前提条件

- Rust 1.75+（[rustup](https://rustup.rs) 推奨）
- Linux / macOS（TUI は crossterm raw mode に依存）

### インストールとデプロイ

```bash
# 1. クローン
git clone https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork.git
cd LsmAgentEmergentWork

# 2. ワンコマンドビルド（cargo build --release → ./laew をルートにコピー）
./rebuild_restart_app.sh

# 3. バージョン確認
./laew --version
# laew 0.1.0 (build 2026-09-08 xx:xx:xx CST, git xxxxxxx)
```

### LLM プロバイダ設定

```bash
# Anthropic プロバイダを追加（5 タプル：protocol + provider_name + model_name + end_point + api_key）
./laew provider add --protocol anthropic \
    --provider-name myAnthropic --model-name claude-sonnet-5 \
    --end-point https://api.anthropic.com --api-key sk-ant-xxxx

# OpenAI プロバイダを追加
./laew provider add --protocol openai \
    --provider-name myOpenAI --model-name gpt-5 \
    --end-point https://api.openai.com --api-key sk-xxxx

./laew provider list          # 一覧（* = 使用中）
./laew provider use 2         # 使用中モデルを切り替え
./laew provider delete 2      # レコード削除
```

> エンドポイント自動補完：Anthropic は `v1/messages`、OpenAI は `chat/completions` を自動付与。
> 設定ファイルなし —— すべて**ルートディレクトリ**の SQLite（`LsmAgentEmergentWork.db`）に保存。

### 3 つの利用モード

```bash
./laew                                  # TUI マルチターン会話に入る
./laew -p "現在のディレクトリのファイル一覧" # 単一ターンモード
./laew -f /path/to/prompt.md            # ファイルからプロンプトを読み取り実行（絶対/相対パス）
```

### TUI スラッシュコマンド

| コマンド | 動作 |
|------|------|
| `/help` (h, ?) | ヘルプ表示 |
| `/exit` (quit, q) | TUI 終了 |
| `/clear` (c) / `/new` (n) | 履歴クリア、新規 Session 開始 |
| `/model` | 現在のモデル表示 |
| `/provider` | プロバイダ管理（デフォルトは list スクリーン） |

---

## 🔑 キーコンセプト

| コンセプト | 説明 |
|------|------|
| **ルートディレクトリ** | `laew` バイナリのあるディレクトリ；データベースとビルド産物がここに |
| **作業ディレクトリ** | `laew` 起動時のディレクトリ；Agent のファイル/コマンド操作のデフォルトコンテキスト |
| **プロバイダレコード** | 5 タプル：protocol + provider_name + model_name + end_point + api_key；複数設定可、1 つがアクティブ |
| **エンドポイント補完** | Anthropic → `{end_point}/v1/messages`；OpenAI → `{end_point}/chat/completions`；末尾 `/` 自動除去 |
| **プロトコル差異** | Anthropic は `tools[].{name,description,input_schema}`；OpenAI は `tools[].{type:"function",function:{name,description,parameters}}` |
| **Agent-Context** | 各 Agent の独立したリアルタイムコンテキスト（メッセージフロー + 状態）、メモリ状態、ライフサイクル = 現在のユニット |
| **Agent-Memory** | 各 Agent の独立したメモリレイヤー、SQLite `agent_memory` に永続化、ユニット/セッションを跨いで再利用 |
| **SessionContext 要約** | 各タスク完了後に `session_memory` に Markdown 要約を書き込み；Yolo が次回実行時に最新 N 件を自動注入 |

---

## 📁 プロジェクト構造

```
LsmAgentEmergentWork/
├── src/
│   ├── main.rs              # CLI エントリ (clap): TUI / -p / -f / provider サブコマンド
│   ├── lib.rs               # ライブラリエクスポート
│   ├── session.rs           # Session: デバイスフィンガープリント + Session ID + 独立コンテキスト
│   ├── error.rs             # 統一エラータイプ (thiserror)
│   ├── build.rs             # LAEW_BUILD_TIME / LAEW_GIT_HASH を注入
│   ├── config/              # ルート/作業ディレクトリ解析 + SQLite CRUD
│   ├── database/            # schema / models / paths / provider
│   ├── agent/
│   │   ├── mod.rs           # プロトコ非依存ループ: run_session → complete → tool_calls
│   │   ├── orchestrator.rs  # MultiAgentOrchestrator 総編成
│   │   ├── yolo.rs          # YoloRunner デュアルエージェント編成 + 3 ティア分類
│   │   ├── plan.rs          # Plan Agent
│   │   ├── main_work.rs     # Main-Work Agent
│   │   ├── subagent.rs      # SubAgent-Work Agent
│   │   ├── quality.rs       # Quality-Check Agent
│   │   ├── session_context.rs # SessionContext Agent
│   │   ├── profile.rs       # AgentProfile + work_profile() / yolo_profile()
│   │   ├── context.rs       # Agent-Context 独立リアルタイムコンテキスト
│   │   ├── memory.rs        # Agent-Memory 永続メモリ
│   │   ├── json_repair.rs   # JSON 自動修復チェーン
│   │   ├── project_context.rs # プロジェクト文書 5 レベルチェーン発見 + セッション毎注入
│   │   ├── system_prompt/   # SystemPrompt 組み合わせとレンダリング
│   │   ├── permissions/     # パーミッション制御: dangerous / sensitive
│   │   ├── sandbox_hook/    # サンドボックックフック
│   │   └── tools/           # Tool trait + ToolRegistry + 6 ツール
│   ├── llm/
│   │   ├── mod.rs           # 統一メッセージモデル + LlmClient trait
│   │   ├── anthropic.rs     # Anthropic wire 変換
│   │   ├── openai.rs        # OpenAI wire 変換
│   │   ├── sse.rs           # SSE ストリーム解析
│   │   └── resilient.rs     # LLM コール自動レジリエンスレイヤー
│   └── tui/
│       ├── mod.rs           # REPL メインループ + Screen スタック
│       ├── engine.rs        # CLI レンダリングエンジン: Screen trait + Frame
│       ├── form.rs          # 汎用 Tab フォームステートマシン
│       ├── input.rs         # 単一行入力 + インラインヒント + 補完
│       ├── completion.rs    # スラッシュコマンド補完エンジン
│       ├── theme.rs         # ANSI カラー / mask_key マスキング
│       └── screen/          # ProviderList / ProviderForm / ProviderDel サブスクリーン
├── docs/                    # ナレッジベース: 14 ラウンドのディープリサーチ / 82+ ディメンション / 635 gap
├── testReport/              # 自動テストレポート + run_e2e.sh
├── tmpPlan/                 # コーディング中の一時計画（コミットなし）
├── scripts/                 # ヘルパースクリプト（mock_llm_server.py）
├── rebuild_restart_app.sh   # ワンコマンドリビルド
├── CLAUDE.md                # Agent エントリドキュメント + 膨大な教訓
└── AGENTS.md                # エンジニアリングエントリドキュメント（= CLAUDE.md）
```

完全な設計は `docs/` 下の各専門ドキュメントを参照。

---

## 🧪 自動テスト —— Agent 自己検査端末

2 つのテストレイヤー：

| レイヤー | エントリ | 用途 |
|-------|------|------|
| ユニットテスト | `cargo test` | Rust 関数レベルカバレッジ（モジュール、解析、変換、ツール） |
| E2E (CLI) | `bash testReport/run_e2e.sh` | mock LLM、`-p` / `provider` / protocol wire / プロジェクトコンテキスト注入 / TUI サブスクリーン tmux 自動化 を実行 |

### TUI サブスクリーン自動化（tmux control-mode）

```bash
# バックグラウンドセッションで TUI を起動、100x30 に固定
tmux new-session -d -s laew_e2e -x 100 -y 30 ./laew
# キー送信
tmux send-keys -t laew_e2e -l "/provider list"
tmux send-keys -t laew_e2e Enter
# パネルのキャプチャとアサーション
tmux capture-pane -p -t laew_e2e | grep -F "/provider list"
# クリーンアップ
tmux kill-session -t laew_e2e
```

> 詳細は `docs/TUI自動化テスト/` を参照。

---

## 📚 14 ラウンドのディープリサーチ —— Agent プログラミングのナレッジベース

`docs/` には **14 ラウンドのディープリサーチ** が蓄積され、**82+ ディメンション** をカバー、
**635 の laew gap**（L1–L635）を累積、すべて Agent 産出：

| ラウンド | テーマ | 規模 |
|------|------|------|
| ラウンド 1–6 | アーキテクチャ / マルチターン / Context / ツール / メモリ / Workflow / Yolo / QC / MCP / Skill / Protocol wire / SubAgent / Goal / TUI / Hook | ~160k 行 |
| ラウンド 7 | ファイル編集 / コード検索 / Git / Bash / マルチモーダル / PromptCaching / Schema / WebFetch | ~10k 行 |
| ラウンド 8 | Telemetry / Session / Tool パーミッション / LSP / Hook / Skill / マルチテナント / TUI | ~13.5k 行 |
| ラウンド 9 | CrashDump / WebUI / OAuth / i18n / Release / WebSocket / コンテナ / CRDT | ~9.4k 行 |
| ラウンド 10 | 15 メインドキュメントに新章追加 | ~27k 行 |
| ラウンド 11 | Agent 協力 / ストリーミング / エラーハンドリング / テスト / 設定 / プラグインエコシステム / プロトコル翻訳 / システムプロンプト | ~30k 行 |
| ラウンド 12 | HTTP クライアント / セキュリティ防御 / モデルルーティング / データ移行 / パフォーマンス / ログ / CLI / 状態永続化 | ~17k 行 |
| ラウンド 13 | ローカル推論 / KV cache / GUI 自動化 / OS / ベンチマーク / パラダイム比較 / DSL / WebAssembly | ~14.9k 行 |
| ラウンド 14 | 8 つの新ディメンション + 200 の新 gap | ~9.4k 行 |

> 専門コレクション索引：`docs/专题/专题-第十三轮深挖合集.md` など。
> 実装進捗台帳：`docs/专题/专题-laew实现进度对照表.md`。

---

## 🤝 フォロー＆サポート

このプロジェクトが面白いと思ったら、各プラットフォームのアカウントをフォローして、完全な開発・デモ動画をご覧ください：

| プラットフォーム | 検索アカウント |
|------|---------|
| Kuaishou（快手） | **封刀灌海** |
| Douyin（抖音） | **封刀灌海** |
| Bilibili（B站） | **封刀灌海** |
| Xiaohongshu（小红书） | **封刀灌海** |
| WeChat Video（微信视频号） | **封刀灌海** |

---

## ☕ 寄付

サーバー費用、LLM API コール、ナレッジベース蓄積には継続的なコストがかかります。このプロジェクトが役に立った場合や面白いと思った場合、寄付をお願いします：

| WeChat Pay | Alipay |
|:--------:|:----------:|
| ![WeChat QR](ProjectPic/微信二维码.jpg) | ![Alipay QR](ProjectPic/支付宝二维码.jpg) |

> `ProjectPic/` はリポジトリにコミットされているため、GitHub / Gitee / GitCode で直接 QR コードを表示できます。

**連絡先**：

- 📱 電話：`13520647302`
- 💬 WeChat：`liushimeng109117198`

---

## 📜 ライセンス

このプロジェクトは **MIT License** でオープンソース —— 詳細は [`LICENSE`](LICENSE) を参照。

> Copyright (c) 2026 LsmAgentEmergentWork Authors
>
> 以下に定める条件に従い、本ソフトウェアおよび関連ドキュメントのファイル（以下「ソフトウェア」）
> の複製を取得するすべての人に対し、ソフトウェアを無制限に扱うことを無償で許可します。
> これには、ソフトウェアの複製を使用、複製、変更、結合、公開、配布、サブライセンス、
> および/または販売する権利、およびソフトウェアを提供する相手に同じことを許可する権利が
> 無制限に含まれます。
>
> 上記の著作権表示および本許可表示は、ソフトウェアのすべての複製または実質的な部分に
> 含まれるものとします。
>
> ソフトウェアは「現状のまま」で、明示であるか暗問であるかを問わず、何らの保証もなく
> 提供されます。ここでいう保証とは、商品性、特定目的への適合性、および非侵害性に
> ついての保証を含みますが、これに限定されません。いかなる場合も、作者または著作権者は、
> 契約行為、不法行為、またはその他であるかを問わず、ソフトウェアまたはソフトウェアの
> 使用またはその他の取引に起因または関連するいかなる請求、損害、またはその他の責任に
> ついて責任を負わないものとします。

すべてのコードは AI Agent 自動記述、人間がレビュー後にコミット。

---

## 🌟 Star / Watch / Fork

このプロジェクトで「Agent プログラミング」について新しい洞察を得た場合は：

- ⭐ **Star** —— より多くの人に Agent プログラミングの力を見ていただく
- 👁️ **Watch** —— 今後のイテレーションをフォロー
- 🍴 **Fork** —— あなたの環境でマルチエージェント CLI プラットフォームを再構築

このリポジトリは 2 つのプラットフォームで同期：

| プラットフォーム | リンク |
|------|------|
| Gitee | `https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork` |
| GitCode | `https://gitcode.com/liusm109117198/LsmAgentEmergentWork` |

> 💡 **14 ラウンドのディープリサーチ / 635 gap / 100% Agent 自動プログラミングのアプローチがあなたにインスピレーションを与えた場合は**、
> [Issues](https://gitee.com/liushimeng109117198_admin/LsmAgentEmergentWork/issues) であなたのチームの類似の実践を共有してください。
> 一つの ⭐ は、この動きを前進させるために 10 のブログ記事よりも効果的です。

**これは一人の人間が書いたコードではありません。AI Agent チームが 24 時間休まずプログラミングした作品です。**

---

**バージョン**：v0.1.0  |  **最終更新**：2026-09-08  |  **ビルド**：Agent 自動ビルド
