# Oxen Build Cheatsheet

這份 cheatsheet 說明如何在本 repo build debug 與 release binary。

## Prerequisites

建議先安裝 repo 需要的開發工具：

```bash
bin/install-prereqs
```

Rust toolchain 由 `rust-toolchain.toml` 指定。一般 shell 直接使用下面的 `cargo` 指令；如果你是在 Codex/RTK 環境中執行，請在指令前加上 `rtk`。

## Binary 對照

| Package | Binary | Debug path | Release path |
|---|---|---|---|
| `oxen-cli` | `oxen` | `target/debug/oxen` | `target/release/oxen` |
| `oxen-server` | `oxen-server` | `target/debug/oxen-server` | `target/release/oxen-server` |

## Build Debug Binary

Debug build 適合本機開發與快速迭代，產物會放在 `target/debug/`。

Build 兩個主要 binary：

```bash
cargo build -p oxen-cli -p oxen-server
```

只 build CLI：

```bash
cargo build -p oxen-cli
```

只 build server：

```bash
cargo build -p oxen-server
```

確認 binary 可執行：

```bash
./target/debug/oxen --help
./target/debug/oxen-server --help
```

直接用 Cargo 跑 debug binary：

```bash
cargo run -p oxen-cli -- --help
cargo run -p oxen-server -- --help
```

## Build Release Binary

Release build 會開啟最佳化，產物會放在 `target/release/`。

Build 兩個主要 binary：

```bash
cargo build -p oxen-cli -p oxen-server --release
```

只 build CLI：

```bash
cargo build -p oxen-cli --release
```

只 build server：

```bash
cargo build -p oxen-server --release
```

確認 release binary 可執行：

```bash
./target/release/oxen --help
./target/release/oxen-server --help
```

## Production Release Build

部署用 build 建議使用 `production` feature。這會啟用 metrics、OpenTelemetry，以及 `liboxen` 的 production feature。

```bash
cargo build --workspace --release --features production
```

如果只要 build 兩個主要 binary 並帶 production feature：

```bash
cargo build -p oxen-cli -p oxen-server --release --features production
```

## 常用檢查

檢查目前 build 出來的 binary 版本：

```bash
./target/debug/oxen --version
./target/release/oxen --version
```

清掉舊 build artifacts 後重新 build：

```bash
cargo clean
cargo build -p oxen-cli -p oxen-server
cargo build -p oxen-cli -p oxen-server --release
```

如果要看比較多 debug log，可以在執行 binary 時加上 `RUST_LOG`：

```bash
RUST_LOG=debug ./target/debug/oxen --help
RUST_LOG=warn,liboxen=debug ./target/debug/oxen-server --help
```
