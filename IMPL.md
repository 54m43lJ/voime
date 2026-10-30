# IMPL — 开发实现记录

本文记录按 `README` / `USER` / `CORE` / `PROVIDER` / `BUILD` / `ASR` 实现 Voime 时发现的文档缺失、文档与实现不一致、技术坑以及已修复的 BUG。编号为全局顺序，每条都给出问题、解决方法和文档修订建议；“不动”用于已有其它条目覆盖同一章节时，避免重复修改。

## 一、开发中发现文档的错误

### 1. CORE 文件树缺失 `asr/features.rs`

**1. 问题描述**

`CORE §1` 的 `voime-core` 文件树没有 `asr/features.rs`，但 `CORE §5.4/§5.5` 的调用栈和 `ASR §3.1/§5.2` 都明确引用 `asr::features::Fbank`。按文件树创建模块时，开发者无法确定 Fbank 应该放在哪里。

**2. 解决方法**

- 新增 `crates/voime-core/src/asr/features.rs`，提供 `Fbank`、`StreamingFbank` 和 `FbankConfig`。
- `Fbank` 负责整段音频的 fbank；`StreamingFbank` 负责 VAD 的滑动窗口 fbank，两者共用 16 kHz、80 bin、25 ms 窗长、10 ms 帧移、32768 样本缩放的参数。
- 在 `asr.rs` 中导出该模块，并为帧数、空音频和 80 bin 增加测试。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §1 File tree | 增 | 在 `asr/firered_ctc.rs` 前增加 `asr/features.rs`。 |
| CORE.html | §4.8 附近 | 增 | 补充 `Fbank`、`StreamingFbank`、`FbankConfig` 的签名小节。 |
| ASR.html | §5.2 代码落点 | 不动 | 已说明 `asr/features.rs` 已存在，见第 1 条。 |

### 2. `Config` 缺少模型清单，启动职责与调用栈不一致

**1. 问题描述**

`CORE §4.2` 的 `Config` 只有 `model_dir`、`input_device`、`threads`、`socket`，但 `CORE §4.12` 的 `Pipeline::spawn(config, factory, audio_rx)` 只能拿到 `Config`，而加载 VAD/CTC/Punc 需要 `ModelManifest`。如果按原签名实现，要么 pipeline 再次读取并校验 manifest，要么在文档之外增加参数；同时 `CORE §5.1` 又把 `Config::resolve` 和 `ModelManifest::load/verify` 写成两个独立步骤。

**2. 解决方法**

- 给 `Config` 增加 `pub manifest: ModelManifest`。
- `config::resolve` 内部完成 `ModelManifest::load`、`verify`、`rebase`，并在失败时返回带模型路径的错误。
- `Pipeline::spawn` 直接使用 `config.manifest`，不再重复读取文件。
- `config::Args` 承载 `model_dir`、`input_device`、`threads`、`socket`、`verbose`，由 `voimed` 的 `Cli` 转换而来。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.2 config.rs | 改 | `Config` 增加 `manifest: ModelManifest`，并写明 `resolve` 负责加载、校验与路径重定位。 |
| CORE.html | §5.1 启动 | 改 | 启动调用栈改为 `config::Config::resolve(args)` 一次完成 manifest 加载/校验，移除重复的独立 load/verify 步骤。 |
| CORE.html | §4.12 pipeline.rs | 不动 | `Pipeline::spawn(&config, ...)` 保持原签名，manifest 来源见第 2 条。 |

### 3. FireRedVad 的签名与批处理调用栈矛盾

**1. 问题描述**

`CORE §4.6` 写的是 `FireRedVad::push(frame: &[f32]) -> f32`，暗示一次只处理一个 VAD 帧并返回单个概率；但 `CORE §5.3` 的调用栈是 `FireRedVad::push(batch)` 后逐个 `handle_frame(frame, audio)`。实际 VAD ONNX 输入是 `[1, T, 80]` 的 fbank 批次，同时还要维护 `caches_in/caches_out` 状态，单帧签名既不符合模型输入，也会放弃文档要求的 100 ms 批处理。

**2. 解决方法**

- 实现为 `push(samples: &[f32]) -> anyhow::Result<Vec<VadFrame>>`，一次处理一批 16 kHz 音频并返回每帧的 `speech_prob` 与 `is_speech`。
- 保留 `reset()` 清空 streaming fbank 和所有状态张量。
- Pipeline 以 100 ms 为批次调用 VAD，再按帧送入分段器。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.6 vad/firered.rs | 改 | 改为批量 `push(samples) -> Vec<VadFrame>`，补充 `VadFrame` 和 `VadConfig`。 |
| CORE.html | §5.3 Continuous 监听 | 不动 | 已按批次和逐帧处理描述，本条不重复修改。 |

### 4. Segmenter 的帧类型、结束原因命名与实际实现不一致

**1. 问题描述**

`CORE §4.7` 的 `Segmenter::push(frame: &[f32], ...)` 把 VAD 帧写成裸 `f32` 切片，且事件原因命名为 `EndReason`；实际实现使用 `VadFrame`、`SegmentEndReason`，还需要 `is_active()` 判断当前是否有活动段。文档没有说明 `MIN_SPEECH_MS`、`MIN_SILENCE_MS`、`PTT_AUTO_END_MS` 这些工程常量由谁拥有。

**2. 解决方法**

- 定义 `SegmentEndReason::{Silence, MaxDuration}` 和 `SegmenterEvent`。
- `push` 接收 `VadFrame`，按 `Ptt/Toggle` 与 `Continuous` 使用不同静音阈值。
- 增加 `is_active()`、`active_samples()`、`force_end()`、`reset()`；常量放在 segmenter 模块，文档只给推荐值。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.7 vad/segmenter.rs | 改 | 使用 `VadFrame`、`SegmentEndReason`，补充 `is_active` 和常量归属说明。 |
| CORE.html | §5.3 Continuous 监听 | 不动 | 分段调用顺序不变，本条不重复修改。 |

### 5. Pipeline 接口与调用栈不一致，缺少运行时模式切换

**1. 问题描述**

`CORE §4.12` 把 `spawn` 写成 `Pipeline::spawn`，但 `CORE §5.1` 的调用栈使用 `pipeline::spawn`；同时 `PipelineCommand` 只有 Start/Finish/Shutdown，无法把 addon 的 `Mode` 变化传给 worker，`PipelineEvent` 也没有致命错误通道。这样 daemon 在配置重载或 pipeline 异常时没有合法路径处理。

**2. 解决方法**

- 实现为自由函数 `pipeline::spawn(config, factory, audio_rx)`。
- `Pipeline` 暴露 `events` 接收端，增加 `set_mode(mode)`。
- `PipelineCommand` 增加 `SetMode(Mode)`，`PipelineEvent` 增加 `Fatal(String)`。
- daemon 在模式变化且当前段空闲时发送 `SetMode`；pipeline 致命错误通过 `Fatal` 上报并触发 daemon 退出。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.12 pipeline.rs | 改 | `spawn` 改为自由函数，补充 `SetMode`、`set_mode`、`Fatal`。 |
| CORE.html | §5.1 启动 | 改 | 统一使用 `pipeline::spawn`。 |

### 6. Daemon 与 IpcServer 的签名不足以支撑事件循环

**1. 问题描述**

`CORE §4.15` 的 `Daemon::new(config, pipeline, audio)` 没有 IPC 服务参数，`on_client(message)` 也没有客户端 ID；`CORE §4.16` 的 `IpcServer::next_event(&mut self)` 无法与 pipeline 事件一起做 `select`，且没有 `send_to`、角色管理和关闭接口。按原文实现会遇到“daemon 无法定向回错、无法保证只有一个 Fcitx 客户端、无法优雅退出”的问题。

**2. 解决方法**

- `Daemon::new(config, pipeline, audio, ipc)`；`on_server_event` 处理连接、消息、坏消息和断线。
- `IpcServer` 增加 `events()` 返回事件接收端、`send_to`、`set_role`、`role`、`shutdown`。
- 定义 `ServerEvent::{Connected, Message, BadMessage, Disconnected}`，断线事件携带客户端 ID 和角色。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.15 daemon.rs | 改 | `new` 增加 `IpcServer`，补充 `on_server_event` 与客户端 ID。 |
| CORE.html | §4.16 ipc_server.rs | 改 | 增加 `events`、`send_to`、`set_role`、`role`、`shutdown` 和 `ServerEvent`。 |
| CORE.html | §5.1 启动 | 不动 | 启动顺序不变，接口变化见本条。 |

### 7. `ServerMessage::Status` 与 `State::status` 未定义

**1. 问题描述**

`CORE §4.13` 的 `ServerMessage::Status { /* ... */ }` 没有字段，`CORE §4.14` 的 `State` 也没有把当前状态组装成 `Status` 的方法。TUI 的 `ctl status` 和 doctor 无法知道应该依赖哪些字段。

**2. 解决方法**

- 定义 `Status { recording, finalizing, segment_id, mode, hotkey, addon, transcript_count }`。
- `State::status(recording, finalizing, segment_id)` 负责组装快照；`ServerMessage::Status { status }` 作为唯一状态查询响应。
- C++ 客户端按同一字段名解析。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.13 voime-ipc | 改 | 用完整 `Status` 结构替换 `Status { ... }`。 |
| CORE.html | §4.14 state.rs | 增 | 补充 `State::status(...) -> Status`。 |

### 8. addon 配置示例无法直接编译和解析枚举

**1. 问题描述**

`CORE §4.22` 的示例只写 `enum class Mode` 和 `Option<Mode>`，但 Fcitx 的 `Option<enum>` 需要对应的 `marshallOption/unmarshallOption`；直接使用裸 enum 不能把 `ptt/toggle/continuous` 写入或读出 RawConfig。示例中的 `Option`、`KeyListOption`、`KeyListConstrain` 也没有命名空间限定，复制到新文件时容易编译失败。

**2. 解决方法**

- 使用 `FCITX_CONFIG_ENUM_NAME(Mode, "ptt", "toggle", "continuous")` 生成字符串映射。
- 使用 `fcitx::Option<Mode>`、`fcitx::KeyListOption`、`fcitx::KeyListConstrain`。
- 在 addon 测试中执行 `config.save(raw)` / `config.load(raw, true)`，验证 `Mode=ptt` 可往返。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.22 config.h/cpp | 改 | 补入 `FCITX_CONFIG_ENUM_NAME` 和完整命名空间限定。 |

### 9. BUILD 写 C++17，但 Fcitx 5.1.23 头文件实际要求 C++20

**1. 问题描述**

`BUILD §1` 要求 C++17。实际使用 Fcitx 5.1.23 的 `fcitx/event.h`、`fcitx-utils/key.h`、`fcitx-utils/log.h` 时会出现 `std::span`、`std::ranges`、`std::string_view::starts_with` 等 C++20 才有的符号，C++17 编译直接失败。

**2. 解决方法**

- CMake 设置 `CMAKE_CXX_STANDARD 20`，`CMAKE_CXX_STANDARD_REQUIRED ON`。
- `BUILD §1` 的 addon 构建依赖改为 C++20。
- 保留 C++17 语法可读性，但不再声明 C++17 兼容。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §1 环境 | 改 | `C++17` 改为 `C++20`，说明由 Fcitx 5.1.23 头文件决定。 |

### 10. addon 安装命令没有指定 Fcitx 前缀

**1. 问题描述**

`BUILD §2/§6` 使用 `cmake -S fcitx-addon -B build/fcitx` 后直接 `sudo cmake --install`。CMake 默认前缀是 `/usr/local`，而发行版 Fcitx 的 addon 通常从 `/usr/lib/fcitx5` 与 `/usr/share/fcitx5` 查找；按原文安装后可能出现“输入法列表没有 Voime”。

**2. 解决方法**

- 构建命令显式指定 `-DCMAKE_INSTALL_PREFIX=/usr`。
- 安装脚本或打包脚本使用同一前缀；自定义前缀时同时确认 Fcitx 的 addon 搜索路径。
- `BUILD §7` 的卸载说明明确删除 Fcitx 前缀下的文件。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §2 构建 | 改 | 增加 `-DCMAKE_INSTALL_PREFIX=/usr`。 |
| BUILD.html | §6 发布检查 | 改 | 同步使用 `/usr` 前缀。 |

### 11. README 的 addon 安装链接指向模型章节

**1. 问题描述**

`README §2` 的“安装 addon”行链接到 `BUILD §3`，但 `BUILD §3` 是“安装模型”；addon 的编译和安装实际在 `BUILD §2`。用户按链接跳转后看不到 `cmake` 安装步骤。

**2. 解决方法**

- 把链接改为 `BUILD.html#b2`。
- 保留 `USER §2` 对运行约定的引用。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| README.html | §2 用户交互 | 改 | “安装 addon”链接改为 `BUILD §2`。 |

### 12. PROVIDER / ASR 的 manifest 示例缺少 `token_count`

**1. 问题描述**

`PROVIDER §4.2` 和 `ASR §5.1` 的 manifest 示例只有 `tokens`，没有 `token_count`；实现中的 `CtcArtifact` 需要该字段来校验词表大小与 ONNX 输出维度。虽然 serde 默认值 8667 能让官方模型通过，但文档没有说明这个约束，替换模型时容易出错。

**2. 解决方法**

- `CtcArtifact` 增加 `token_count`，缺省为 8667。
- `FireRedCtc::new_with_factory` 校验 `CtcTokenizer::token_count()` 与清单值一致。
- `tools/verify_models.py` 从 `tokens.txt` 计算并写入 manifest。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER.html | §4.2 模型包 | 增 | manifest 的 `asr` 增加 `token_count` 字段及含义。 |
| ASR.html | §5.1 Manifest | 增 | 同步示例和 CTC 词表约束。 |

### 13. `Tui::draw` 的 `&App` 参数无法更新滚动与视口

**1. 问题描述**

`CORE §4.20` 写 `Tui::draw(&mut self, app: &App)`，但绘制时需要根据终端尺寸更新 `viewport_height`，并在跟随模式下修正 `scroll.offset`。只读引用会迫使 UI 层绕过 `App` 维护状态，或增加额外的可变全局状态。

**2. 解决方法**

- 改为 `draw(&mut self, app: &mut App) -> anyhow::Result<()>`。
- 在 draw 内更新 `app.viewport_height` 和 `app.scroll.offset`。
- 保留 `Tui::init`、`Tui::restore` 的原语义。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.20 ui.rs | 改 | `draw` 改为 `&mut App` 并返回 `Result`。 |

### 14. CORE 的 addon 文件树与类字段不完整

**1. 问题描述**

`CORE §1` 没有列 `icons/voime.svg`、`third_party/nlohmann/json.hpp`、`tests/addon_test.cpp`；`CORE §4.21` 的 `VoimeAddon` 字段没有 `focusWatcher_`、`reconnectTimer_`、`starting_`、`nextLocalId_`、`socketPath_`，也没有 `onDaemonDisconnected`、`connectDaemon`、`bindTarget`、`clearPreview`。这些正好是断线恢复、目标绑定和焦点清理的实现入口。

**2. 解决方法**

- 文件树补齐 addon 图标、vendored JSON 头和测试文件。
- 类中增加上述字段和辅助方法；文档标明它们属于 addon 生命周期/断线恢复路径。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §1 File tree | 增 | 补 `icons/voime.svg`、`third_party/nlohmann/json.hpp`、`tests/addon_test.cpp`。 |
| CORE.html | §4.21 voime.h/cpp | 改 | 补焦点 watcher、重连 timer、starting 状态、目标/预览辅助方法。 |

### 15. BUILD 的测试覆盖描述超出实际测试

**1. 问题描述**

`BUILD §5` 声称 `tests/daemon.rs` 覆盖 PTT/toggle/continuous/断线、`tests/asr_ctc.rs` 覆盖真实 10–60 s 解码、addon 测试覆盖 keymatch/preview/target。实际实现中，daemon 测试主要覆盖 socket/角色/断线契约；真实 CTC 解码测试需要 `VOIME_CTC_MODEL_DIR` 才执行，否则直接返回；addon 测试覆盖 mode、热键字符串、IPC 枚举和空 target，不覆盖真实 Fcitx keymatch/preview 渲染。

**2. 解决方法**

- 保留不依赖模型权重的测试，避免普通 `cargo test` 下载 740 MiB 模型。
- 文档按“默认测试 / 需要模型或图形环境的 opt-in 测试”分层描述。
- 后续可把 daemon 状态机拆出可注入的 pipeline 接口，补真实 PTT/toggle/continuous 测试。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §5 测试 | 改 | 按实际覆盖重写清单，标明 `VOIME_CTC_MODEL_DIR` 等 opt-in 条件。 |

## 二、开发中发现文档没写明的细节

### 16. IPC 的 JSON 包装、枚举大小写和协议版本没有写死

**1. 问题描述**

`USER §4.3` / `CORE §4.13` 只给出了 Rust 枚举形状，没有说明线上 JSON 是外部标签还是内部标签、`Mode`/`Role`/`TriggerSource` 是否小写、`Phase`/`AddonState` 的字段值是什么、协议版本号是多少、`Hello.config` 是否可选。C++ addon 和 Rust daemon 只要任一侧猜错就无法握手。

**2. 解决方法**

- 统一使用外部标签：`{"Hello":{"role":"fcitx","version":1,"config":{...}}}`。
- `Mode`、`Role`、`TriggerSource`、`Phase`、`AddonState` 使用小写值；`ErrorCode` 使用 snake_case。
- 定义 `PROTOCOL_VERSION = 1`，双方版本不一致时返回 `VersionMismatch`。
- 在 `voime-ipc` 中增加 JSON 往返测试，并把示例写入文档。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| USER.html | §4.3 IPC | 增 | 增加一条完整 JSON 示例和大小写/版本约定。 |
| CORE.html | §4.13 voime-ipc | 增 | 明确 serde 外部标签、枚举值和 `PROTOCOL_VERSION`。 |

### 17. workspace 根目录放 `tests/` 需要额外包配置

**1. 问题描述**

`CORE §1` 同时写了 workspace 根 `Cargo.toml` 和根目录 `tests/`，但没有说明虚拟 workspace 没有可运行的 package target；直接把 `tests/` 放在虚拟 workspace 根下，`cargo test` 不会自动发现这些测试。

**2. 解决方法**

- 根 `Cargo.toml` 同时作为 workspace 和 `voime-tests` 测试包，设置 `autotests = false`，显式声明 `[[test]]` 路径。
- 测试通过 dev-dependencies 引用 `voime-core`、`voime-ipc`、`voimed`。
- `voimed` 增加 `src/lib.rs`，让集成测试可以导入 `state`、`ipc_server`、`daemon`。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §1 File tree | 增 | 说明根 `Cargo.toml` 也是测试包，`crates/voimed/src/lib.rs` 存在。 |
| BUILD.html | §5 测试 | 增 | 说明 `cargo test` 如何发现根目录集成测试。 |

### 18. `Library=voime` 与 `voime.so` 的文件名约定

**1. 问题描述**

`PROVIDER §4.8` 要求 addon 配置 `Library=voime`，`BUILD §2` 要求产物为 `voime.so`；但 CMake 默认给共享模块加 `lib` 前缀，生成 `libvoime.so`。如果文件名和配置不一致，Fcitx 会报 addon 加载失败，且现有系统 addon 多数使用 `Library=libfoo`，更容易误判。

**2. 解决方法**

- CMake 设置 `set_target_properties(voime PROPERTIES PREFIX "")`，产物固定为 `voime.so`。
- 安装到 `${CMAKE_INSTALL_LIBDIR}/fcitx5/voime.so`，描述文件保持 `Library=voime`。
- 用 `nm -D` 确认导出 `fcitx_addon_factory_instance_voime`。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §2 构建 | 增 | 说明必须关闭 `lib` 前缀，产物名为 `voime.so`。 |
| PROVIDER.html | §4.8 addon 描述 | 增 | 明确 `Library=voime` 对应 `voime.so`，不是 `libvoime.so`。 |

### 19. systemd unit 的安装位置和自定义前缀没有写明

**1. 问题描述**

`BUILD §4` 直接让用户执行 `systemctl --user enable --now voime.service`，但没有说明 unit 应安装到 `~/.config/systemd/user/voime.service`，也没有说明 `systemd/voime.service` 里的 `ExecStart=%h/.local/bin/voimed` 在自定义 `--prefix` 时会失效。按原文档部署到 `/usr/local` 或其它目录时，服务会启动失败。

**2. 解决方法**

- 增加 `scripts/install_voimed.sh`，安装二进制后生成 `~/.config/systemd/user/voime.service`，把 `ExecStart` 替换为实际 `$BINDIR/voimed`。
- 支持 `--prefix`、`--bindir`、`--no-build`、`--no-systemd`。
- `BUILD §7` 的卸载步骤明确删除该 unit 文件。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §4 运行 | 增 | 写明 unit 路径、安装脚本和 `systemctl --user daemon-reload`。 |
| BUILD.html | §7 升级/卸载 | 改 | 卸载时删除 `~/.config/systemd/user/voime.service`。 |

### 20. `models/manifest.json` 只是结构模板，不能直接运行

**1. 问题描述**

`CORE §1` 说明 `models/` 只保留 manifest 结构、权重由安装脚本生成，但没有说明仓库内模板的 SHA-256 是全零、`asr/`、`vad/`、`punc/` 目录可能不存在；直接运行 `voimed` 会因模型校验失败退出。新开发者容易把“结构模板”当成可运行模型包。

**2. 解决方法**

- 保留模板 manifest，明确它只描述目录和张量契约。
- `scripts/install_models.sh` 下载/导出后重新生成真实 SHA-256 和 manifest。
- 启动错误信息包含缺失文件和期望路径。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §1 File tree | 不动 | 已有权重不提交的 note，本条不重复改文件树。 |
| BUILD.html | §3 安装模型 | 增 | 明确模板 manifest 不可直接启动，必须先运行安装脚本。 |

### 21. `--source-dir` 不覆盖 CTC 模型

**1. 问题描述**

`BUILD §3` 提供 `scripts/install_models.sh --source-dir /path/to/official/weights`，容易被理解为所有模型都可以离线。实际 `--source-dir` 只提供 FireRedPunc 权重和 VAD ONNX/CMVN；FireRedASR2-CTC 仍然从官方 tar.bz2 下载，或使用 `~/.cache/voime/ctc/` 的缓存。

**2. 解决方法**

- 文档明确 `--source-dir` 只覆盖 Punc/VAD 与 FireRedASR2S 源码。
- CTC 需要网络下载，或提前把官方 tar.bz2 放入缓存目录。
- 当前安装脚本通过 `download_ctc` 优先复用缓存，失败时再走 Hugging Face。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §3 安装模型 | 改 | 说明 `--source-dir` 不包含 CTC，列出 CTC 缓存位置或预置方式。 |

### 22. addon 重连策略只有“退避”一词

**1. 问题描述**

`CORE §5.1` 只说 addon 重连失败时“按退避间隔重试”，没有给出初始间隔、上限或是否需要指数退避；实现目前是固定 2 秒 timer。不同实现者可能做成 50 ms 忙等或 30 分钟不可用。

**2. 解决方法**

- 当前实现使用固定 2 s 重连 timer；连接成功后 timer 保留但不再动作。
- 断线时立即清理预览、目标、按键状态，热键恢复为普通按键。
- 后续如需指数退避，在 timer 回调里维护失败次数，文档再同步。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §5.1 启动 | 改 | 写明固定 2 s 重连或指数退避的具体参数。 |
| PROVIDER.html | §4.7 消息与错误 | 增 | 说明 `DaemonUnavailable` 的重连行为。 |

### 23. continuous 的有界队列并不能真正隔离慢 ASR

**1. 问题描述**

`CORE §2` 说 continuous 完成段进入容量 2 的队列，队列满时丢弃最旧段，以避免阻塞当前语音。实际 pipeline 只有一个 worker，VAD、CTC、标点串行执行；worker 在 finalize 期间无法处理新的音频，音频仍会在容量 2048 的 cpal 通道里堆积，超过容量后丢失。队列主要覆盖“已完成但尚未识别”的段，不能实现真正的采集/识别并行。

**2. 解决方法**

- 保持单 worker 的简单性，文档只承诺“有界待识别队列”，不承诺音频不丢。
- 队列容量为 2，满时丢最旧完成段；音频通道满时由 cpal 侧丢弃。
- 如果后续要求长时连续输入不丢音频，需要独立 ASR 线程或磁盘/内存环形缓冲，另开 issue。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §2 CONVENTIONS | 增 | 说明队列只覆盖完成段，音频通道仍是独立背压点。 |
| CORE.html | §5.3 Continuous 监听 | 增 | 增加“单 worker 串行，不保证慢识别时音频零丢失”的说明。 |

### 24. 连续/TUI 启动时的目标输入框绑定时机

**1. 问题描述**

`USER §2` 说每个语音段绑定“语音段开始时聚焦的输入框”。实际 PTT/Toggle 由 addon 在按键事件里立即绑定；continuous、TUI/ctl 启动时，addon 要等 daemon 广播 `SegmentStarted` 后才读取 `mostRecentInputContext()`。如果 VAD 起点和广播之间焦点切换，绑定的上下文可能略晚于语音起点。

**2. 解决方法**

- PTT/Toggle 保持按键时立即绑定。
- continuous/TUI 在 `SegmentStarted` 时绑定当前焦点，之后录音期间不再改变。
- 后续如需严格绑定 VAD 起点，可让 daemon 在 `SegmentStarted` 中附带 addon 侧无法获取的时序信息，或由 addon 在收到第一帧音频时提前绑定（当前无该事件）。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| USER.html | §2 CONVENTIONS | 改 | 说明 continuous/TUI 的绑定发生在 `SegmentStarted` 广播时，而非 VAD 内部起点。 |
| CORE.html | §2 CONVENTIONS | 增 | 同步 addon 的目标绑定时机。 |

### 25. `voime doctor` 只检查默认模型目录和固定 addon 路径

**1. 问题描述**

`USER §4.2` 只提供 `voime doctor [--socket PATH]`，但 `voimed` 支持 `--model-dir`，addon 也可能安装在自定义前缀。当前 doctor 固定检查 `~/.local/share/voime/models` 和 `/usr`、`/usr/local` 下的 addon 文件，无法诊断自定义部署。

**2. 解决方法**

- 当前实现按文档只接受 `--socket`，模型目录取默认值。
- doctor 检查 socket 权限、daemon 快照、manifest/SHA、音频设备、fcitx5 进程、addon/inputmethod/voime.so 与用户配置。
- 后续给 doctor 增加 `--model-dir` 和 `--prefix`，或从 daemon 快照返回运行中的模型目录。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| USER.html | §4.2 命令行 | 增 | 说明 doctor 当前只检查默认模型目录，或增加 `--model-dir`。 |
| BUILD.html | §4 运行 | 增 | 在自定义前缀部署时说明 doctor 的路径限制。 |

### 26. `segment_id` 的所有权没有写明

**1. 问题描述**

`USER §4.3` 的 `Start/Stop/Toggle` 都带 `segment_id`，`SegmentStarted` 也会广播 `segment_id`，但没有说明 ID 由客户端生成还是 daemon 分配。如果 addon 和 TUI 各自生成，可能冲突；如果 daemon 重新分配，客户端如何更新也不明确。

**2. 解决方法**

- daemon 是 `segment_id` 的唯一分配者，内部递增 counter。
- 客户端可发送 0 或本地临时 ID 作为提示；daemon 在 `SegmentStarted` 中广播最终 ID。
- addon 收到 `SegmentStarted` 后更新 `segmentId_` 和 `target_.segment_id`；TUI 只显示服务端状态。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| USER.html | §4.3 IPC | 增 | 明确 daemon 分配 segment_id，客户端只做提示。 |
| CORE.html | §4.15 daemon.rs | 增 | 说明 allocate_segment 与 SegmentStarted 广播。 |

## 三、其它技术上踩的坑

### 27. Fcitx `watchEvent` 的模板参数是 `EventType` 值，不是事件类

**1. 问题描述**

Fcitx 的接口是 `watchEvent<EventType::InputContextKeyEvent>(phase, callback)`，模板参数需要一个 `EventType` 枚举值；写成 `watchEvent<fcitx::InputContextKeyEvent>` 会报 “not a member of fcitx”。同样，`InputContextFocusOut` 也要写成 `EventType::InputContextFocusOut`。

**2. 解决方法**

- 使用 `instance_->watchEvent<fcitx::EventType::InputContextKeyEvent>(PreInputMethod, ...)`。
- 回调参数分别使用 `fcitx::KeyEvent &` 和 `fcitx::FocusOutEvent &`。
- 用 `FCITX_UNUSED` 处理未使用的 entry/event，避免 `-Wunused`。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §5.1 启动 | 增 | 写成 `EventType::InputContextKeyEvent`，并给出 KeyEvent/FocusOutEvent 回调类型。 |

### 28. 在 Fcitx 事件回调中销毁 EventSource 不安全

**1. 问题描述**

IpcClient 的 socket 断线回调运行在 Fcitx 事件循环线程中。如果在 `onReadable` 内部直接 `ioEvent_.reset()` 销毁当前事件源，事件循环可能在回调返回后继续访问已释放对象；如果只关闭 fd 不处理事件源，也可能反复触发错误事件。

**2. 解决方法**

- 断线时先关闭 fd，再对 `ioEvent_` 调用 `setEnabled(false)`，不立即销毁。
- 下一次 `connect` 重新创建 `ioEvent_` 时，旧事件源在回调外安全析构。
- 析构函数同样先 disable 再关闭 fd。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.24 ipc.h/cpp | 增 | 增加事件源生命周期与断线清理顺序说明。 |

### 29. ONNX metadata 的 CMVN 编码格式不固定

**1. 问题描述**

`PROVIDER §4.2` 只说 CMVN 从 ONNX metadata 的 `cmvn_mean` / `cmvn_inv_stddev` 读取，没有说明值可能是 JSON 数组，也可能是空格/逗号分隔的浮点串。只支持一种格式会在部分导出模型上失败。

**2. 解决方法**

- `parse_cmvn` 同时接受 JSON 数组和空白/逗号/分号分隔格式。
- 校验 mean 与 inv_stddev 长度一致且非空。
- 缺少 metadata 时启动失败，错误信息包含缺失的 key。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER.html | §4.2 模型包 | 增 | 说明 CMVN metadata 的可能编码和解析要求。 |
| ASR.html | §3.2 推理 | 增 | 补充 CMVN 解析的容错约定。 |

### 30. CTC greedy 中 blank 对 `prev` 的更新

**1. 问题描述**

CTC greedy 的规则是“逐帧 argmax、合并连续重复、去掉 blank”，但 `prev` 必须在每一帧更新，包括 blank 帧。如果只在非 blank 时更新 `prev`，`a blank a` 会被错误合并成一个 `a`；这是 CTC 解码最常见的实现错误之一。

**2. 解决方法**

- 每帧先求 argmax，然后判断 `id != 0 && id != prev` 才输出。
- 无论是否输出，最后都执行 `prev = id`。
- 单元测试覆盖重复帧、blank 分隔重复、连续 blank 三种情况。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| ASR.html | §3.2 推理 | 增 | 明确 `prev` 每帧更新，包括 blank。 |
| CORE.html | §2 CONVENTIONS | 增 | 将 CTC 规则写成可测试的伪代码或约束。 |

### 31. Fbank 的整段计算与 VAD 流式计算不能共用同一个实例

**1. 问题描述**

VAD 需要保留滑动窗口和未消费样本，CTC 的 final/partial 每次只处理一个完整音频段，要求从零开始计算。如果两者共用同一个 `Fbank` 实例，前一段的窗口状态会污染后一段；另外 fbank 输入需要乘 32768，`snip_edges=true` 会丢弃不足一帧的尾部，文档只写了参数，没有写生命周期。

**2. 解决方法**

- 拆成 `StreamingFbank`（VAD）和 `Fbank`（CTC final/partial）。
- `Fbank::compute` 每次创建独立的 online feature，喂完整段并 `input_finished` 后取帧。
- 统一 `SAMPLE_SCALE=32768`、`snip_edges=true`、`high_freq=0`、`low_freq=20`，并加帧数测试。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| ASR.html | §3.1 预处理 | 增 | 明确 `Fbank` 与 `StreamingFbank` 的生命周期和样本缩放。 |
| CORE.html | §2 CONVENTIONS | 增 | 说明 VAD 流式特征与 CTC 整段特征不能共享实例。 |

## 四、后续 ISSUES 修复的 BUG

本节记录开发过程中已由后续修复提交解决的 BUG；仍未完成的事项已在前三节标注为后续 issue。

### 32. 标点模型校验把 `token_type_ids` 填成了随机 token id

**1. 问题描述**

`tools/verify_models.py` 的 `feeds_for` 用 `"ids" in name` 判断是否填 token id，结果同时匹配 `input_ids` 和 `token_type_ids`。`token_type_ids` 被填入 100–1000 的随机值，BERT 的 token type embedding 只有 0/1，运行时报 `idx=600 must be within the inclusive range [-2,1]`，导致 `install_models.sh` 在 punc 校验阶段失败。

**2. 解决方法**

- 只有 `input_ids` 填随机词表 id。
- `token_type_ids` 固定为 0，`attention_mask` 固定为 1。
- 标点校验输入从 1 个 token 改为 8 个 token，覆盖真实序列长度。
- 用已安装的 Punc 模型验证输出 `[1, 8, 5]`，并重新跑完整 `install_models.sh` 通过。
- 修复提交：`9e660ca`。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| PROVIDER.html | §4.2 模型包 | 增 | 明确 punc 的 `token_type_ids` 是 0/1 句段 id，不能填 token id。 |
| BUILD.html | §5 测试 | 不动 | 测试清单见第 15 条，避免重复修改。 |

### 33. CTC 缓存包在重新解压前被删除

**1. 问题描述**

`tools/install_models.py` 的 `download_ctc` 先把官方 tar.bz2 下载到 `cache/ctc/`，随后用 `shutil.rmtree(root.parent)` 清理旧目录，结果把刚下载的压缩包一起删除，再执行 `extract_archive` 时找不到文件。该问题在下载缓存已存在时不一定暴露，清理缓存后重装会失败。

**2. 解决方法**

- 只删除解压目标 `root`，保留 `cache/ctc/*.tar.bz2`。
- 解压前检查目标目录，缺失时重新解压。
- 安装脚本优先复用缓存，失败时再走 Hugging Face 备用地址。
- 修复已随初始实现提交 `359cc8a` 合入。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| BUILD.html | §3 安装模型 | 增 | 说明压缩包会保留在缓存中，重装不会重新下载。 |

### 34. 断线事件在移除客户端后丢失角色信息

**1. 问题描述**

IpcServer 的 reader 线程在发送 `Disconnected` 事件前先把客户端从 `clients` map 移除；daemon 收到事件后再调用 `role(client_id)` 已经查不到角色，于是无法判断断开的是 Fcitx 还是 TUI，也就无法正确把 `addon` 标记为 disconnected、停止 continuous 自动分段。

**2. 解决方法**

- `ServerEvent::Disconnected` 直接携带 `role: Option<Role>`。
- reader 在移除 map 条目的同时取出 role，再发送事件。
- daemon 用事件里的 role 判断 Fcitx 断线，并广播新的 Snapshot。
- 修复已随初始实现提交 `359cc8a` 合入。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §4.16 ipc_server.rs | 增 | `ServerEvent::Disconnected` 增加 `role` 字段。 |
| CORE.html | §5.7 断线 | 不动 | 行为描述已存在，见第 34 条。 |

### 35. addon 在 daemon 确认前设置 recording，Busy/Error 后状态卡死

**1. 问题描述**

addon 最初在按键命中后立即把 `recording_` 设为 true，再发送 `Start`。如果 daemon 因已有语音段返回 `Busy`、或因没有输入上下文返回错误，addon 会收到 Error，但 `recording_` 已经为 true，后续热键会被 `recording_` 分支拦住，表现为“热键失灵直到重启 Fcitx”。

**2. 解决方法**

- 增加 `starting_` 状态：按键后只置 `starting_`，收到 daemon `State.recording=true` 后才置 `recording_`。
- 收到 Error 且当前未 recording 时，清理 `starting_`、`holdActive_`、`toggleCode_`、目标引用和预览。
- Toggle 在 `starting_` 期间忽略重复启动；PTT 松开时发送 Stop，由 daemon 幂等处理。
- 在 `onDaemonDisconnected` 中一并重置 `starting_`。
- 修复已随初始实现提交 `359cc8a` 合入。

**3. 修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| CORE.html | §5.2 热键与模式 | 增 | 增加 `starting_` 状态机，说明收到 State 前不置 recording。 |
| CORE.html | §4.21 voime.h/cpp | 不动 | 字段清单见第 14 条，避免重复修改。 |
