# Codex 二进制构建与部署记录（Windows）

> 目的：本仓库 MonkeyCode 需要可运行的 `codex` 二进制作为第二引擎
> （`codex app-server`）。本文记录从源码构建并部署到本机的全过程，供复现。

## 交付物

| 项 | 值 |
|---|---|
| 二进制 | `C:\Users\12090\AppData\Roaming\com.teemocode.desktop\codex\bin\codex.exe` |
| 版本 | `codex-cli 0.0.0`（源码主分支，未打版本 tag） |
| 源码 | `D:\works\ziji\codex\codex`（openai/codex 仓库） |
| SHA-256 | `8972dd34b2985da8534aedacfcc61a44170084c7d44402edd2a3af7aeba1bb06` |
| 构建 profile | debug（未优化，1305 MB，仅用于开发/集成测试） |

## 为什么需要自建

公开渠道无法获取 codex Windows 二进制：`@openai/codex-win32-x64` 是**私有 npm 包**
（公开 registry 404）；GitHub releases 无公开二进制；官方 install.sh 为空。

## 构建环境（本次实测可复现）

- **Rust 工具链**：`rustup` 用户级安装，toolchain `stable-x86_64-pc-windows-gnu`
  （GNU 目标；MSVC 目标在本机缺 MSVC 链接器，改用 GNU）。
- **MinGW binutils**：`C:\Users\12090\msys64\mingw64\bin\`（gcc 16.2 + dlltool 2.47）。
  **必须加入 PATH**，否则编译 `windows-sys`/`getrandom` 时报
  `error calling dlltool 'dlltool.exe': program not found`。
- **网络**：可访问 crates.io（首次需下载全部依赖）。

## 构建步骤

```bash
# 1. 准备环境（关键：把 MinGW binutils 加进 PATH）
export PATH="/c/Users/12090/msys64/mingw64/bin:$HOME/.cargo/bin:$PATH"

# 2. 进入 codex 的 Rust workspace
cd /d/works/ziji/codex/codex/codex-rs

# 3. 构建 CLI（产物含 codex / codex-app-server 等所有子命令）
cargo build -p codex-cli --target x86_64-pc-windows-gnu
# 产出：target/x86_64-pc-windows-gnu/debug/codex.exe（约 9 分钟，debug）

# 4. 部署到 MonkeyCode 约定目录
mkdir -p "C:/Users/12090/AppData/Roaming/com.teemocode.desktop/codex/bin"
cp target/x86_64-pc-windows-gnu/debug/codex.exe \
   "C:/Users/12090/AppData/Roaming/com.teemocode.desktop/codex/bin/codex.exe"

# 5. 校验
sha256sum "C:/Users/12090/AppData/Roaming/com.teemocode.desktop/codex/bin/codex.exe"
```

## 已验证能力

- `codex --version` → `codex-cli 0.0.0`
- `codex app-server --listen stdio://` 可启动（收到 `initialize` 后进入就绪；
  **注意**：必须先创建 `CODEX_HOME` 指向的目录，否则报
  `CODEX_HOME points to "...", but that path does not exist`）。
- `codex exec --json` 存在（JSONL 输出）。

## 已知注意点

1. **debug 二进制巨大（1305 MB）**：仅用于开发/集成测试。发布需 `cargo build
   --release` 或官方渠道的 release 二进制。
2. **app-server 完整握手协议未抓全**：本项目仅验证它能启动并响应；`initialize`
   之后的 `thread/start`/`turn/run` 精确报文需开发 agent 在集成时按
   `AGENT-BRIEF.md` Phase 0 抓取。
3. **构建平台一致性**：此二进制用 **GNU** triple 构建。MonkeyCode 接入时作为子
   进程调用，与 OhMyAgent 同机制，不要求与主程序同编译链。

## 复现时若失败

- `dlltool not found` → 确保 MinGW binutils 在 PATH。
- `CODEX_HOME ... does not exist` → 先 `mkdir` 该目录。
- 依赖下载慢 → 配置国内 crates 镜像（如 rsproxy）。
