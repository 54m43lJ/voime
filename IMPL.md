# IMPL — 开发实施与文档修订清单

本清单记录本轮 Windows 前端适配与 GPU 安装开发中的心得。每个主题只在一个大块中出现；修订建议里的“不动”用于避免在多个文档之间重复定义或互相覆盖。动作只约束该条目自身的修改范围：同一章节可能被多个条目引用，按各条内容合并执行；后一条的“不动（本主题）”不撤销其它条目在该章节的增改。“不动（已写明）/（已更新）”表示目标文档已经包含该内容，无需再改。

## 1. 开发中发现文档的错误

### 1.1 Windows `TrayCommand` / `send_command` 的文档签名与实现不一致

1. **问题描述**

`frontends/windows/CORE.html` §4.8 把 `TrayCommand` 写成 `ShowSettings / StartDaemon / StopDaemon / Diagnose / Quit`，并把 `send_command` 写成返回 `anyhow::Result<()>`；实际实现只让 `TrayCommand` 承载设置窗口转交的 `StartDaemon / StopDaemon`，菜单动作走内部 `TrayEvent`，`send_command` 返回 `bool`。按文档实现会引入未使用的枚举分支和错误的调用约定。

2. **解决方法**

- 以实际线程边界为准：菜单命令在窗口回调写入 `TrayEvent`，由 `poll_events` 在主线程执行。
- `TrayCommand` 只保留启动 / 停止服务；`send_command` 返回是否投递成功。
- 更新 CORE §4.8 的签名和说明，不伪造 `ShowSettings / Diagnose / Quit` 分支。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.8 | 不动（已更新） | 已改为 `StartDaemon / StopDaemon`；`send_command` 返回 `bool`；已说明 `TrayEvent` 承载菜单动作。 |
| `frontends/windows/USER.html` | §4.6 | 不动（已写明） | 设置窗口通过托盘命令启动 / 停止服务的语义不变。 |

### 1.2 Windows CORE 列出的 `on_focus_change` 在源码中不存在

1. **问题描述**

`frontends/windows/CORE.html` §4.1 / §5.3 把 `on_focus_change` 作为前端方法描述（失焦时隐藏浮层、保留 `pending_target`、停止 PTT / Toggle），但源码只在 `tick` 内直接做 50 ms 焦点检查，没有该方法。按文档定位代码会找不到入口，也无法确认 pending 目标的处理是否按文档执行。

2. **解决方法**

- 在 `Frontend` 中补 `on_focus_change`，由 `tick` 在 50 ms 节流点调用；行为保持原文档。
- `on_target_lost` 继续负责 pending 目标与 Stop 的具体处理，避免两处逻辑分叉。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.1、§5.3 | 不动（已实现） | 文档保留 `on_focus_change`；源码已补该方法并由 `tick` 调用。 |
| `frontends/windows/src/main.rs` | — | 已完成 | 增加 `on_focus_change`，焦点检查节流 50 ms。 |

## 2. 开发中发现文档没写明的细节

### 2.1 托盘“最近错误码”的展示位置、格式与生命周期

1. **问题描述**

`frontends/windows/USER.html` §4.4 要求托盘状态显示“最近错误码”，但没有说明从哪条消息取、显示在托盘提示还是菜单、使用 Debug 名还是 snake_case、成功重连后是否清空。

2. **解决方法**

- 前端新增 `last_error: Option<&'static str>`，收到 `ServerMessage::Error` 时记录 `ErrorCode` 的 snake_case 名称。
- 托盘菜单在连接状态下方增加禁用项“最近错误：<code>”，不写 tooltip，不记录识别文本。
- 成功重连不主动清空，保留到下一次错误，便于用户回看。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.4 | 增 | 明确最近错误码显示在托盘菜单、使用 snake_case、成功重连不清空。 |
| `frontends/windows/CORE.html` | §4.1、§4.8 | 增 | 增加 `last_error` 字段与 `last_error()`；托盘菜单展示。 |

### 2.2 登录启动 Run 键的写入时机与幂等性

1. **问题描述**

`frontends/windows/CORE.html` §4.2 只写“固定使用 `HKCU\...\Run\Voime`”，没有说明托盘每次启动是否重写、安装脚本何时写、值相同是否跳过。实现若每次启动都重写，会制造无意义的持久化写，也会放大安全软件的持久化行为检测。

2. **解决方法**

- `Config::set_start_at_login` 先读现有字符串，与当前 exe 的 `"path"` 比较，相同则直接返回。
- 安装脚本同样只在 Run 值不同时写入；卸载仍删除同名值。
- 文档明确“值相同不写”，避免把幂等实现误读为缺少写入。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.2 | 增 | 写 Run 键前先比较现有值，相同不重写。 |
| `BUILD.html` | §3.2 | 增 | 安装脚本仅当 Run 值变化时写入。 |
| `packaging/windows/README.md` | 安装 | 不动（已更新） | 已写明 Run 值不同时才写入。 |

### 2.3 安装 / 升级时托盘正在运行的处理与安装后是否启动

1. **问题描述**

文档只写安装脚本复制文件、写 Run 键，没有说明升级时托盘是否先退出、复制被占用文件失败怎么办、安装后是否自动拉起托盘。

2. **解决方法**

- 安装脚本先向运行中的托盘发送 `WM_CLOSE`（窗口类 `VoimeTrayWindow`），等待最多 3 秒；仍未退出则 `Stop-Process` 兜底，再复制文件。
- 脚本不自动启动新托盘；由用户手动启动或下次登录启动，避免安装器形成“复制 + 写 Run 键 + 立即执行”的连续行为。
- 卸载前同样先停止托盘 / daemon，再删除文件与 Run 键。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §3.2、§3.4 | 不动（已更新） | §3.2 已写明升级先关闭托盘、超时强停、安装后不自动启动；§3.4 沿用同一流程。 |
| `frontends/windows/USER.html` | §4.6 | 不动（已更新） | 已写明升级前关闭托盘、安装后不自动启动。 |
| `packaging/windows/README.md` | 安装 | 不动（已更新） | 已写明 `WM_CLOSE` + force fallback 与不自动启动。 |

### 2.4 构建资源：manifest 嵌入与 `rc.exe` 缺失时的行为

1. **问题描述**

`packaging/windows/README.md` 只列出 `voime-tray.manifest` 和图标路径，没有说明它们是否嵌入 exe、依赖哪个工具、缺少工具时是否失败。原 `build.rs` 只嵌入图标，manifest 实际未使用。

2. **解决方法**

- `build.rs` 找到 `rc.exe` 时把 `voime.ico` 和 `voime-tray.manifest` 一起写入 `.rc` 并链接；`cargo:rerun-if-changed` 跟踪两者。
- 找不到 `rc.exe` 时打印 warning 并跳过资源，不影响运行期 DPI：入口仍调用 `SetProcessDpiAwarenessContext(PerMonitorV2)`。
- 文档写明“SDK 可用时嵌入，否则 warning 跳过”。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `packaging/windows/README.md` | Manifest | 不动（已更新） | 已写明 `build.rs` 在 `rc.exe` 可用时嵌入 manifest / icon，缺失时 warning 跳过；运行期 DPI 兜底。 |
| `frontends/windows/CORE.html` | §4.1 | 增 | 说明 `build.rs` 资源嵌入与 `rc.exe` 依赖。 |

### 2.5 `config.json` 的 UTF-8 / BOM 契约

1. **问题描述**

文档只规定设置载体为 `%APPDATA%\voime\config.json`，没有说明编码 / BOM。测试中用 PowerShell `Set-Content -Encoding utf8` 生成带 BOM 的文件，托盘读取失败并静默退出，排查成本高。

2. **解决方法**

- 明确配置文件是无 BOM UTF-8 JSON；工具写配置时不得带 BOM。
- 人工测试或脚本生成配置时使用 `UTF8Encoding($false)` 或写入 ASCII 字段。
- 不在前端增加 BOM 容错（保持严格解析），但在文档中写明排障点。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 | 增 | 写明 `config.json` 为无 BOM UTF-8；BOM 会导致解析失败。 |
| `frontends/windows/CORE.html` | §4.2 | 增 | 写明加载 / 保存使用无 BOM UTF-8。 |

## 3. 其它技术上踩的坑

### 3.1 Windows 集成测试使用 Unix-only `Endpoint::from_path`

1. **问题描述**

`tests/daemon.rs` 直接用 `Endpoint::from_path(tempdir/.../voimed.sock)`；Windows 的 `Endpoint` 只有 `NamedPipe(String)`，`from_path` 不存在，`cargo test --workspace` 在 Windows 直接编译失败。

2. **解决方法**

- 测试用 `test_endpoint(label)` 辅助：Unix 用 tempdir + `from_path`，Windows 用进程号 + 原子计数生成唯一管道名 + `from_name`。
- 两个用例共用同一套端点创建逻辑，避免再次写死平台路径。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `CORE.html` | §2（测试）、§5（验证） | 增 | 平台 transport 测试必须通过 `Endpoint` 抽象创建端点，不得直接调用 Unix-only API。 |
| `tests/daemon.rs` | — | 已完成 | 增加 `cfg(unix)` / `cfg(windows)` 端点辅助。 |

### 3.2 GPU 源码构建的三连坑：libclang、UTF-8、vendor 静态库

1. **问题描述**

在 Windows 上从源码构建 Qwen worker 先后失败三次：`bindgen` 找不到 `libclang.dll`；MSVC 把 llama.cpp 源文件里的 UTF-8 `↵` 按本地代码页解释，报 C2001；链接阶段 `llama-common` 引用 `cpp-httplib` / `vendor-hash`，但 `llama-cpp-sys-4` 没有把这两个 vendor 库加入链接。

2. **解决方法**

- `install-gpu.ps1` 检测 `LIBCLANG_PATH` / LLVM / VS LLVM；缺失时默认把 LLVM MSI 以 `msiexec /a` 解压到 `%LOCALAPPDATA%\Programs\LLVM`（仅保留 `libclang.dll` 与 `lib\clang\<ver>\include`），无需管理员；`-LlvmScope system` 走 UAC 安装；`-NoLlvmInstall` 只提示。
- 构建前设置 `CFLAGS` / `CXXFLAGS` 追加 `/utf-8`，覆盖 llama.cpp 的 UTF-8 字符。
- 新增 `crates/voime-gpu-qwen3/build.rs`：Windows 下从 `%LOCALAPPDATA%\llcb` 选最新的 `cpp-httplib.lib` / `vendor-hash.lib`，补充 link-search 与 link-lib；其他平台不输出。
- 同时检测 Vulkan SDK（`VULKAN_SDK` 或 `C:\VulkanSDK\<ver>`，需要 `glslc.exe`）与 CUDA Toolkit（`CUDA_PATH` / `nvcc.exe`），缺失时在构建前失败并给出安装提示；预编译 worker 走 `-SourceDir/bin/`（Windows 为 `bin\voime-gpu-qwen3.exe`）可跳过全部源码构建。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §3.3 | 不动（已更新） | 已写明 Windows 源码构建依赖 LLVM / Vulkan SDK / CUDA Toolkit；脚本自动装 LLVM（user / system），Vulkan / CUDA 由用户安装；`/utf-8` 与 vendor 链接。 |
| `crates/voime-gpu-qwen3/USER.html` | §4.4 | 不动（已更新） | 已增加 `-LlvmScope` / `-NoLlvmInstall` 与源码构建依赖说明。 |
| `crates/voime-gpu-qwen3/CORE.html` | §1 | 不动（已更新） | 文件树已增加 `build.rs`，并说明补齐 vendor 静态库。 |

### 3.3 Defender 把安装链识别为 `Behavior:Win32/Persistence.A!ml`

1. **问题描述**

安装脚本复制新 exe、写 `HKCU\...\Run\Voime` 后立即启动 `voime-tray.exe`，被 Defender 行为检测命中；随后托盘 exe 被隔离、Run 键被删除。旧脚本没有自动启动步骤，所以未触发。

2. **解决方法**

- 移除安装后自动启动；安装只复制 + 写 Run 键。
- Run 键改为仅值变化时写入（见 2.2），减少重复持久化写。
- 不关闭 Defender、不加排除项；本地未签名构建的误报通过签名 + 向 Microsoft 提交解决。
- 文档记录该误报与合规处理方式。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §3.2 | 不动（已更新） | 已补充未签名构建的 Defender 误报说明；安装后不启动见 2.3。 |
| `packaging/windows/README.md` | 安装 | 不动（已更新） | 已记录误报名称与合规处理；停止 / 启动流程见 2.3。 |
| `frontends/windows/CORE.html` | §4.9 | 不动（重叠见 2.3） | 安装后不启动的契约已在 2.3 写明，此处不重复。 |

### 3.4 误判 VAD 长停顿：耳机漏音让 VAD 一直判 speech

1. **问题描述**

Windows Toggle 会话中，停说话后等待十几秒仍不出 Final，浮层停在 Partial。最初怀疑 `AUTO_END_SILENCE_MS` 对短停顿不敏感，改了 core 的 segmenter / pipeline；实测是耳机 / 扬声器声音被 Fifine 麦克风轻微拾取，`speech_prob` 持续 ≥ 0.5，`silence_run_ms` 永远到不了 800 ms，daemon 不产生 `SegmentReady`。core 在 Linux 端已端到端验证无此问题。

2. **解决方法**

- 回退 segmenter / pipeline 改动；不改 core 识别逻辑。
- 复现方法：用 cpal 录一段“说话 + 静音”的麦克风原始音频，离线跑 VAD，观察静音段 `speech_prob` / `is_speech`；本机静音录音里仍出现长 SPEECH 段，确认是漏音 / 回声。
- 规避属于音频环境问题：使用耳机时降低漏音、关闭外放 / 调低音量、换指向性麦克风或启用系统回声消除；不要通过改 core 阈值解决。
- 文档补充“耳机 / 扬声器漏音可能导致 VAD 不结束”的排障提示。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.5 | 增 | 在覆盖 / 错误表增加“耳机漏音 / 回声导致 VAD 持续判语音、无 Final”的排障说明。 |
| `crates/voime-core/USER.html` | §3 | 不动（本主题） | core 的 VAD 分段契约不变；漏音属于采集环境，排障说明放 Windows USER。 |
