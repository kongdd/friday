# autossh / Friday 语音助手 — Agent 指南

本仓库是 **Rust workspace**：用 `autossh-core` 守护系统 `ssh` 端口转发，用 `autossh-ui`（egui）编辑配置并看日志，用 **`friday` crate** 接收并播放 TTS，同时录制、暂停及回放本地音频。Agent 改代码时应保持三者职责分离，避免在 UI 里再抄一份 Friday 实现。

## 仓库结构

| 路径                             | Crate / 产物   | 职责                                                                                                    |
| -------------------------------- | -------------- | ------------------------------------------------------------------------------------------------------- |
| `crate/core`                     | `autossh-core` | TOML 配置、热重载、`ssh` 子进程监督与退避重连；可选 Windows Service                                     |
| `crate/friday`                   | `friday`（库） | HTTP 接收、mpv 播放，以及麦克风录制、暂停、结束与本地回放                                               |
| `crate/ui`                       | `autossh-ui`   | 原生 GUI：连接编辑、监督器、日志、Friday 接收、录音及全局 F8；**依赖** `friday`，不要新增 `ui/src/friday.rs` |
| `docs/`                          | —              | 隧道与部署说明（`docs/README.md`、`GOAL.md`）                                                           |
| `config.toml`                    | —              | 开发用示例；默认用户配置为 `~/.config/autossh/config.toml`                                              |
| `.pi/skills/autossh-ui-preview/` | —              | Linux 下 Xvfb + 截图验证 egui UI                                                                        |

工作区根 `Cargo.toml`：`members = ["crate/core", "crate/friday", "crate/ui"]`；release 配置为体积优化（`opt-level = "z"`, LTO, `panic = "abort"`）。

## 常用命令

```bash
# 全工作区
cargo build
cargo build --release

# 按 crate
cargo build -p autossh-core
cargo build -p autossh-ui
cargo test -p friday
cargo test -p autossh-core

# 校验配置
cargo run -p autossh-core -- check --config path/to/config.toml
```

Windows 服务脚本：仓库根 `service.ps1`（包装 `sc.exe`）；详细步骤见 `docs/README.md`。

## 架构约定

1. **SSH 不内嵌**：core 只 `Command` 启动 `ssh` / `ssh.exe`，BatchMode、ExitOnForwardFailure、keepalive 等见 `docs/README.md`。
2. **Friday 与隧道解耦**：Friday 由 GUI 拥有；托盘隐藏时接收与录音仍可运行；停止接收只释放 **17322** 并杀接收端 mpv，不影响录音或 SSH 监督器。
3. **mpv 播放**（`crate/friday`）：
   - 静默：`configure_player_command`（`--force-window=no`、`--osd-level=0`、`--no-terminal` 等）；Windows 用 `CREATE_NO_WINDOW`；Unix 用 `process_group(0)`。
   - Stop：`PlaybackRegistry` 登记子进程；`FridayReceiver::stop()` 与 `serve` 退出时 `kill_all()`。
   - 可选环境变量 `FRIDAY_MPV` 指定 mpv 可执行文件路径。
4. **UI 模块**：`crate/ui/src/app/` 按面板拆分（`connections`、`supervisor`、`centre` 含 Friday、`logs`、`modal_host`）；`logic` 与 `ui` 分离以支持 Windows 托盘在窗口隐藏时仍处理命令。

## 配置与端口

- **默认配置路径**：`autossh_core::default_config_path()` → ① `<exe-dir>/config.toml`（若存在）② `~/.config/autossh/config.toml`。`home_config_path()` 返回标准 XDG 位置供 ensure_config 使用。
- **Friday 监听**：`friday::LISTEN_ADDR` = `127.0.0.1:17322`；健康检查 `GET /`、`/health`、`/ping`；播报 `POST /speak`（JSON：`type: "mp3"`、`data` base64、`rate` 0.5–2.0）。
- **Friday 录音**：Windows 使用 CPAL，Linux 调用 `arecord`（可由 `FRIDAY_ARECORD` 指定）；全局 `F8` 开始/结束录音；WAV 按 `YYYYMMDD_NNN.wav` 保存至可执行文件旁的 `media/`，仅本地回放，发送端尚未实现。
- Windows 上 Friday 重启监听需在 bind 前 `SO_REUSEADDR`（`friday` 的 `socket2` 路径），避免 10048。

## 编码与风格

- **Rust edition 2024**；保持 crate 边界清晰，公共 API 在 `lib.rs` 再导出（如 `FridayReceiver`、`FridayRecorder`、`LISTEN_ADDR`）。
- **极简**：一次做好一件事；有意义注释保留（尤其平台差异、Windows 服务账户、端口复用）。
- **测试**：friday 的 payload 校验在 `http.rs` 单元测试；core 有 `tests/unit/`。
- **截图 / UI 回归**：用 skill `autossh-ui-preview`（隔离临时 HOME 与 `config.toml`，勿动用户真实 `~/.ssh`）。

## 文档与产品方向

- 根 `README.md`：Friday 语音助手 checklist（隧道、播放、待办「下达 tasks」）。
- 隧道细节以 `docs/README.md` 为准；设计动机见 `docs/GOAL.md`。
- 生成物、截图放 `images/`；markdown 用相对路径引用。

## Agent 修改时注意

- 改 Friday 播放或 Stop 行为 → **只改 `crate/friday`**，确认 `autossh-ui` 仍 `use friday::...`。
- 改连接/重载语义 → `crate/core` + 必要时 UI 的 `supervisor` / `modal`。
- 跨平台：条件编译 `cfg(windows)` / `cfg(unix)` 与 `target.'cfg(...)'` 依赖（如 ui 的 eframe features、friday 的 socket2）需一并考虑。
- 不要删除解释平台坑或未来复用的注释；凌乱注释可整理，不要空删。
