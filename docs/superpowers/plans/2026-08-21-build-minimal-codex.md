# 构建最小化 Codex 引擎（精简版）

## 目标
创建一个仅包含 agent 核心功能的 Codex 二进制，体积接近 OhMyAgent（~23MB）。

## 策略
1. 只编译 `app-server` 和它直接依赖的核心 crate
2. 禁用 TUI、MCP server、exec-server、cloud tasks 等重型模块
3. 使用 release profile + 优化

## 步骤

### 1. 创建最小化构建脚本

```bash
# 清理旧的构建
rm -rf target/x86_64-pc-windows-gnu/release/codex*
rm -rf target/x86_64-pc-windows-gnu/release/deps/codex_*

# 只构建 app-server（不含 CLI 外壳）
export PATH="/c/Users/12090/msys64/mingw64/bin:$HOME/.cargo/bin:$PATH"

cargo build -p codex-app-server --release \
  --target x86_64-pc-windows-gnu \
  --bin app-server \
  --features "minimal-agent" 2>&1

# 如果没有 minimal-agent feature，先构建默认 release
cargo build -p codex-app-server --release \
  --target x86_64-pc-windows-gnu \
  --bin app-server 2>&1
```

### 2. 检查产物

```bash
ls -la target/x86_64-pc-windows-gnu/release/app-server.exe
```

### 3. 如果产物仍太大，继续裁剪

查看 app-server 的具体依赖树：
```bash
cargo tree -p codex-app-server --target x86_64-pc-windows-gnu -e features 2>/dev/null | head -50
```

## 注意事项
- 原 `codex.exe` CLI 是外壳，核心在 `app-server`
- 我们只需要 `app-server --listen stdio://` 子命令
- 可通过 Cargo features 禁用 TUI、MCP 等模块
